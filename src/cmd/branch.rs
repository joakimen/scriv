//! `scriv branch` — list, select, and check out git branches.
//!
//! Local and remote branches are shown in one list, coloured by where they
//! live. Selecting a remote-only branch creates the local branch and sets its
//! upstream.
//!
//! Every listing arrives ordered by [`git::by_relevance`]: the default branch,
//! then the current one, then local, then remote-only, newest first within
//! each.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use anyhow::{Result, bail};

use crate::git::{self, Branch, BranchKind, Filter};
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

    // No second verb: what else there is to do with a branch — deleting it —
    // is destructive, and a key beside the one that checks it out is the wrong
    // distance from a mistake.
    let chosen =
        select::select_one_reloading(items(&offered), prompt, &ctx.config.selector, reload, &[])?;
    let name = chosen.value;

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

// --- rm ---------------------------------------------------------------------

/// The branches `rm` may offer: the ones that exist in this clone, minus the
/// one that is checked out.
///
/// Remote branches are deliberately absent. Deleting one is a push, which is
/// not something to be one keystroke and no network round trip away from, and
/// nothing on this list would say it had happened.
fn deletable(branches: &[Branch]) -> Vec<&Branch> {
    branches
        .iter()
        .filter(|branch| branch.kind != BranchKind::Remote && !branch.head)
        .collect()
}

/// The mark a branch carries in the list of what is about to be deleted, and in
/// the selector: whether git considers its commits to have landed.
///
/// A squash merge leaves no trace git can follow — the branch's commits never
/// become ancestors of the commit that merged them — so `not merged` is
/// routinely true of work that is finished. It is a fact, not a warning, which
/// is why it is shown rather than obeyed.
fn merge_mark(merged: bool) -> &'static str {
    if merged { "merged" } else { "not merged" }
}

/// Selector rows for deletion: the branch, when it was last committed to, and
/// whether it has landed. Tinted green where git can see it has.
fn rm_items(branches: &[Branch], merged: &HashSet<String>) -> Vec<SelectItem> {
    let width = name_width(branches);
    let mark_width = "not merged".len();
    branches
        .iter()
        .map(|branch| {
            let landed = merged.contains(&branch.name);
            let label = format!(
                "{name:<width$}  {mark:<mark_width$}  {date}  {subject}",
                name = branch.name,
                mark = merge_mark(landed),
                date = branch.date,
                subject = branch.subject,
            );
            SelectItem::new(label.trim_end(), branch.name.clone())
                .color(if landed { 2 } else { 3 })
                .preview(preview(branch))
        })
        .collect()
}

/// `scriv branch rm [NAME]...` — delete local branches, selecting them when
/// none are named.
///
/// What will go is printed with its merge state before the question is put, as
/// `file prune` does, and answering it is the consent that lets an unmerged
/// branch go: a repository that squashes its merges has no other kind, so a
/// flag guarding them would be a flag typed every time and read never.
pub fn rm(ctx: &Ctx, names: &[String], yes: bool) -> Result<()> {
    let branches = collect(ctx, Filter::Local, false)?;
    let merged = git::merged_branches()?;

    let targets: Vec<String> = if names.is_empty() {
        let offered: Vec<Branch> = deletable(&branches).into_iter().cloned().collect();
        if offered.is_empty() {
            bail!("no other local branches — only the one you have checked out");
        }
        match select::select_many(
            rm_items(&offered, &merged),
            "Delete branches",
            &ctx.config.selector,
        ) {
            Ok(selected) => selected,
            Err(e) if e.is::<select::Cancelled>() => return Ok(()),
            Err(e) => return Err(e),
        }
    } else {
        names.to_vec()
    };
    if targets.is_empty() {
        return Ok(());
    }

    if !confirm_deletion(ctx, &targets, &merged, yes)? {
        return Ok(());
    }

    let mut failed = 0;
    for name in &targets {
        // Forced for a branch git cannot see has landed: the confirmation
        // above showed that state and was answered anyway.
        let force = !merged.contains(name);
        match git::delete_branch(name, force) {
            Ok(()) => println!("Deleted {name}"),
            Err(err) => {
                failed += 1;
                eprintln!("error: {name}: {err:#}");
            }
        }
    }
    if failed > 0 {
        bail!(
            "{failed} of {} branches could not be deleted",
            targets.len()
        );
    }
    Ok(())
}

/// Show what is about to go, and what git knows about each, then ask.
fn confirm_deletion(
    ctx: &Ctx,
    targets: &[String],
    merged: &HashSet<String>,
    yes: bool,
) -> Result<bool> {
    let width = targets.iter().map(|n| n.chars().count()).max().unwrap_or(0);
    let color = ctx.color();

    let mut out = term::Listing::stdout();
    for name in targets {
        let landed = merged.contains(name);
        let row = format!("{name:<width$}  {}", merge_mark(landed));
        if !out.line(&term::paint(&row, if landed { 2 } else { 3 }, color))? {
            return Ok(false);
        }
    }
    out.finish()?;

    match term::Confirm::resolve(yes) {
        term::Confirm::Assumed => Ok(true),
        term::Confirm::Ask => {
            let question = format!(
                "Delete {} {}?",
                targets.len(),
                if targets.len() == 1 {
                    "branch"
                } else {
                    "branches"
                }
            );
            let answer = term::confirm(&question)?;
            if !answer {
                println!("Nothing deleted");
            }
            Ok(answer)
        }
        term::Confirm::Impossible => bail!(
            "no terminal to ask for confirmation on — pass `--yes` to delete without being asked"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{RefLine, classify};

    fn branches() -> Vec<Branch> {
        classify(&[
            RefLine {
                refname: "refs/heads/main".into(),
                head: true,
                symref: String::new(),
                upstream: "origin/main".into(),
                date: "2 hours ago".into(),
                subject: "init".into(),
            },
            RefLine {
                refname: "refs/remotes/origin/main".into(),
                head: false,
                symref: String::new(),
                upstream: String::new(),
                date: "2 hours ago".into(),
                subject: "init".into(),
            },
            RefLine {
                refname: "refs/remotes/origin/feature".into(),
                head: false,
                symref: String::new(),
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
            symref: String::new(),
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

    /// Deleting a remote branch is a push. Nothing on this list would say it
    /// had happened, and no confirmation here is consent to change a remote.
    #[test]
    fn only_local_branches_that_are_not_checked_out_are_offered_for_deletion() {
        let here = branches();
        let offered: Vec<&str> = deletable(&here).iter().map(|b| b.name.as_str()).collect();
        assert_eq!(offered, Vec::<&str>::new(), "main is checked out here");

        let mut branches = branches();
        branches[0].head = false;
        let offered: Vec<&str> = deletable(&branches)
            .iter()
            .map(|b| b.name.as_str())
            .collect();
        assert_eq!(offered, vec!["main"], "a remote branch was offered");
    }

    #[test]
    fn the_deletion_rows_say_what_git_can_see_has_landed() {
        let mut branches = branches();
        branches[0].head = false;
        let merged: HashSet<String> = ["main".to_string()].into_iter().collect();

        let rows = rm_items(&branches, &merged);
        assert!(rows[0].label.contains("merged"), "{}", rows[0].label);
        assert_eq!(rows[0].color, Some(2), "a landed branch is not green");
        assert_eq!(rows[0].value(), "main");

        let rows = rm_items(&branches, &HashSet::new());
        assert!(rows[0].label.contains("not merged"), "{}", rows[0].label);
        assert_eq!(rows[0].color, Some(3));
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
