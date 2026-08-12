//! `scriv worktree` — list and select the working trees of the repository the
//! shell is standing in.
//!
//! Switching to one is a `cd`, which a child process cannot do to its parent,
//! so `sel` prints the path and the fish integration's ctrl-t moves there —
//! the same split as `repo sel`.

use anyhow::{Result, bail};

use crate::git::{self, Worktree};
use crate::path::display_path;
use crate::select::SelectItem;
use crate::{Ctx, select, term};

/// Every working tree of the current repository, in git's order.
fn load(ctx: &Ctx) -> Result<Vec<Worktree>> {
    git::ensure_repo()?;
    let worktrees = git::worktrees()?;
    ctx.log
        .info(&format!("found {} worktrees", worktrees.len()));
    if worktrees.is_empty() {
        bail!("no worktrees found");
    }
    Ok(worktrees)
}

/// The column widths a listing shares, so `worktree ls --status` and the
/// selector draw the same row.
///
/// Widths are counted in characters, not bytes: `{:<width$}` pads by character
/// count, so a byte length would over-pad a non-ASCII row.
struct Columns {
    head: usize,
    tags: usize,
}

impl Columns {
    fn of(worktrees: &[Worktree]) -> Self {
        Self {
            head: worktrees
                .iter()
                .map(|w| w.head_label().chars().count())
                .max()
                .unwrap_or(0),
            tags: worktrees
                .iter()
                .map(|w| w.tags().join(" ").chars().count())
                .max()
                .unwrap_or(0),
        }
    }

    /// Render one row: the current-tree marker, what the tree has checked out,
    /// any state tag, and `path` as the caller wants it shown. The tag column
    /// is dropped entirely when no tree carries one, which is the usual case.
    fn row(&self, worktree: &Worktree, path: &str) -> String {
        let marker = if worktree.current { "*" } else { " " };
        let mut row = format!(
            "{marker} {head:<width$}",
            head = worktree.head_label(),
            width = self.head,
        );
        if self.tags > 0 {
            row.push_str(&format!(
                "  {tags:<width$}",
                tags = worktree.tags().join(" "),
                width = self.tags,
            ));
        }
        row.push_str("  ");
        row.push_str(path);
        row
    }
}

/// `scriv worktree ls` — print the path of each working tree, one per line,
/// home-collapsed unless `absolute`.
///
/// `--status` adds the current-tree marker, what it has checked out, and any
/// `locked` or `prunable` tag. Colour follows the same mapping as the selector
/// and is dropped when stdout is not a terminal.
pub fn ls(ctx: &Ctx, absolute: bool, status: bool) -> Result<()> {
    let worktrees = load(ctx)?;
    let color = ctx.color();
    let columns = Columns::of(&worktrees);

    let mut out = term::Listing::stdout();
    for worktree in &worktrees {
        let path = worktree.path.to_string_lossy();
        let path = display_path(&path, ctx.home_str(), absolute);
        let line = if status {
            let row = columns.row(worktree, &path);
            match worktree.color() {
                Some(index) => term::paint(&row, index, color),
                None => row,
            }
        } else {
            path
        };
        if !out.line(&line)? {
            break;
        }
    }
    Ok(())
}

/// Build the selector rows, tinted by [`Worktree::color`]. Every row's value is
/// the absolute path, so the shell `cd`s without re-expanding `~`.
fn items(worktrees: &[Worktree], home: &str) -> Vec<SelectItem> {
    let columns = Columns::of(worktrees);
    worktrees
        .iter()
        .map(|worktree| {
            let abs = worktree.path.to_string_lossy().into_owned();
            let label = columns.row(worktree, &display_path(&abs, home, false));
            let item = SelectItem::new(label, abs.clone()).preview(select::checkout_preview(&abs));
            match worktree.color() {
                Some(color) => item.color(color),
                None => item,
            }
        })
        .collect()
}

/// `scriv worktree sel` — fuzzy-select a working tree and print its absolute
/// path, which is what a shell needs to `cd` there.
pub fn sel(ctx: &Ctx) -> Result<()> {
    let worktrees = load(ctx)?;
    let choice = select::select_one(
        items(&worktrees, ctx.home_str()),
        "Select a worktree",
        &ctx.config.selector,
    )?;
    println!("{choice}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::select::Preview;

    fn worktrees() -> Vec<Worktree> {
        git::mark_current(
            git::parse_worktrees(
                "worktree /home/u/dev/scriv\n\
                 HEAD 950547ef3af47b2e60406bd23e530bdb1e226c6e\n\
                 branch refs/heads/main\n\
                 \n\
                 worktree /home/u/dev/scriv/.claude/worktrees/feat\n\
                 HEAD 32bb788aa1c04d9ee4d1e5a8b0e0b8d1c2f3a4b5\n\
                 branch refs/heads/feat/x\n",
            ),
            Some(std::path::Path::new("/home/u/dev/scriv")),
            std::path::Path::to_path_buf,
        )
    }

    #[test]
    fn rows_mark_the_current_tree_and_return_its_path() {
        let items = items(&worktrees(), "/home/u");
        assert!(items[0].label.starts_with("* main"), "{}", items[0].label);
        assert!(items[1].label.starts_with("  feat/x"), "{}", items[1].label);
        assert_eq!(items[0].value(), "/home/u/dev/scriv");
        assert_eq!(items[0].color, Some(2), "the current tree is green");
        assert_eq!(items[1].color, None);
    }

    /// The label is what the shell would have to re-expand; the value is not.
    #[test]
    fn rows_show_a_collapsed_path_and_return_an_absolute_one() {
        let items = items(&worktrees(), "/home/u");
        assert!(
            items[0].label.ends_with("~/dev/scriv"),
            "{}",
            items[0].label
        );
        assert_eq!(items[0].value(), "/home/u/dev/scriv");
    }

    #[test]
    fn columns_align() {
        let items = items(&worktrees(), "/home/u");
        assert_eq!(
            items[0].label.find("~/dev/scriv"),
            items[1].label.find("~/dev/scriv"),
            "the path column is ragged: {:?}",
            items.iter().map(|i| &i.label).collect::<Vec<_>>(),
        );
    }

    /// The tag column costs two spaces on every row, which the common
    /// repository — no locked or prunable tree — should not pay.
    #[test]
    fn the_tag_column_appears_only_when_a_tree_carries_a_tag() {
        let plain = worktrees();
        let untagged = Columns::of(&plain).row(&plain[0], "~/p");
        assert_eq!(untagged, "* main    ~/p", "main is padded to `feat/x`");

        let mut tagged = plain;
        tagged[1].locked = true;
        let columns = Columns::of(&tagged);
        assert_eq!(columns.row(&tagged[1], "~/q"), "  feat/x  locked  ~/q");
        assert!(
            columns.row(&tagged[0], "~/p").len() > untagged.len(),
            "the untagged row did not make room for the tag column",
        );
    }

    #[test]
    fn column_width_counts_characters_not_bytes() {
        let rows = git::parse_worktrees(
            "worktree /home/u/café\nHEAD abc\nbranch refs/heads/café\n\
             worktree /home/u/b\nHEAD def\nbranch refs/heads/b\n",
        );
        let label = &items(&rows, "/home/u")[0].label;
        assert!(
            label.starts_with("  café  ~/café"),
            "column padded past the longest name: {label:?}",
        );
    }

    #[test]
    fn preview_reads_the_tree_without_taking_its_index_lock() {
        let items = items(&worktrees(), "/home/u");
        let Some(Preview::Command(cmd)) = &items[0].preview else {
            panic!("expected a command preview");
        };
        assert!(cmd.contains("-C '/home/u/dev/scriv'"), "{cmd}");
        assert!(cmd.contains("--no-optional-locks"), "{cmd}");
        assert!(cmd.contains("--max-count=20"), "unbounded log: {cmd}");
    }
}
