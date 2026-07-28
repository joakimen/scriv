//! `scriv pr` — list, pick, and check out GitHub pull requests.
//!
//! Everything here goes through the `gh` CLI (see [`crate::gh`]), so scriv
//! inherits whatever authentication `gh auth login` set up — including SSO and
//! GitHub Enterprise hosts — and stores no credentials of its own.

use anyhow::{Result, bail};

use crate::Ctx;
use crate::gh::{self, PullRequest};
use crate::pick::{self, PickItem, Preview};
use crate::term;

/// Fetch pull requests, failing with a useful message when there are none.
fn collect(ctx: &Ctx, state: &str, limit: usize) -> Result<Vec<PullRequest>> {
    let prs = gh::list(state, limit)?;
    ctx.log.info(&format!("found {} pull requests", prs.len()));
    if prs.is_empty() {
        // `--state all` would otherwise read "no all pull requests found".
        let scope = if state == "all" {
            String::new()
        } else {
            format!("{state} ")
        };
        bail!("no {scope}pull requests found for this repository");
    }
    Ok(prs)
}

/// Width of the widest PR number, so the `#123` column aligns.
fn number_width(prs: &[PullRequest]) -> usize {
    prs.iter()
        .map(|pr| pr.number.to_string().len())
        .max()
        .unwrap_or(0)
}

/// `scriv pr ls` — print one pull request per line.
///
/// `--status` adds the open/draft/merged/closed tag, the source branch, and the
/// last-updated date. Rows are coloured by state, and colour is dropped when
/// stdout is not a terminal.
pub fn ls(ctx: &Ctx, state: &str, limit: usize, status: bool) -> Result<()> {
    let prs = collect(ctx, state, limit)?;
    let color = term::stdout_color();
    let width = number_width(&prs);

    for pr in &prs {
        let row = if status {
            format!(
                "#{number:<width$}  {tag:<6}  {title}  @{author}  [{head}]  {updated}",
                number = pr.number,
                tag = pr.tag(),
                title = pr.title,
                author = pr.author_login(),
                head = pr.head_ref_name,
                updated = pr.updated_date(),
            )
        } else {
            format!(
                "#{number:<width$}  {title}  @{author}",
                number = pr.number,
                title = pr.title,
                author = pr.author_login(),
            )
        };
        println!("{}", term::paint(&row, pr.color(), color));
    }
    Ok(())
}

/// The preview for a pull request: its heading and description, rendered from
/// data already in memory.
///
/// Deliberately *not* `gh pr view`. That would be prettier — it renders
/// markdown and shows check results — but it is a network round trip (~2s here)
/// per highlighted row, spawned again on every move through the list. skim runs
/// preview commands on a background thread and does not kill non-PTY children,
/// so scrolling would leave a queue of `gh` processes racing to finish. The
/// description is worth showing; it is not worth making the list feel slow.
fn preview(pr: &PullRequest) -> Preview {
    let mut out = term::paint(
        &format!("#{number}  {title}", number = pr.number, title = pr.title),
        pr.color(),
        true,
    );
    out.push('\n');
    out.push_str(&format!(
        "{tag}  @{author}  [{head}]  updated {updated}\n\n",
        tag = pr.tag(),
        author = pr.author_login(),
        head = pr.head_ref_name,
        updated = pr.updated_date(),
    ));

    let body = pr.body.trim();
    if body.is_empty() {
        out.push_str("(no description)");
    } else {
        // GitHub stores CRLF; the pane renders the stray CRs as artefacts.
        out.push_str(&body.replace("\r\n", "\n"));
    }
    Preview::Text(out)
}

/// Build picker rows, tinted by state — green ready, grey draft, magenta
/// merged, red closed. The source branch is part of the label so it is
/// fuzzy-matchable, not just the title.
fn items(prs: &[PullRequest]) -> Vec<PickItem> {
    let width = number_width(prs);
    prs.iter()
        .map(|pr| {
            let label = format!(
                "#{number:<width$}  {title}  @{author}  [{head}]",
                number = pr.number,
                title = pr.title,
                author = pr.author_login(),
                head = pr.head_ref_name,
            );
            PickItem::new(label, pr.number.to_string())
                .color(pr.color())
                .preview(preview(pr))
        })
        .collect()
}

/// Fuzzy-select one pull request and return its number.
fn select(ctx: &Ctx, prs: &[PullRequest], prompt: &str) -> Result<u64> {
    let choice = pick::pick_one(items(prs), prompt, &ctx.config.picker)?;
    choice
        .parse()
        .map_err(|_| anyhow::anyhow!("unexpected picker result: {choice}"))
}

/// `scriv pr pick` — fuzzy-select a pull request and print its number, so it
/// composes with `gh`: `gh pr view (scriv pr pick)`.
pub fn pick(ctx: &Ctx, state: &str, limit: usize) -> Result<()> {
    let prs = collect(ctx, state, limit)?;
    let number = select(ctx, &prs, "Pick a pull request")?;
    println!("{number}");
    Ok(())
}

/// `scriv pr checkout [number]` — check out a pull request's branch, picking
/// one when no number is given. The checkout itself is `gh pr checkout`, which
/// handles fork PRs and sets the upstream.
pub fn checkout(ctx: &Ctx, number: Option<u64>, state: &str, limit: usize) -> Result<()> {
    let number = match number {
        Some(number) => number,
        None => {
            let prs = collect(ctx, state, limit)?;
            select(ctx, &prs, "Check out a pull request")?
        }
    };
    gh::checkout(number)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prs() -> Vec<PullRequest> {
        gh::parse_prs(
            r#"[
            {"number":7,"title":"Add branch picker","author":{"login":"joakimen"},
             "headRefName":"feat/branches","isDraft":false,"state":"OPEN",
             "updatedAt":"2026-07-27T09:12:33Z",
             "body":"Adds a picker.\r\n\r\nSecond paragraph."},
            {"number":123,"title":"WIP","author":{"login":"someone"},
             "headRefName":"wip","isDraft":true,"state":"OPEN",
             "updatedAt":"2026-07-20T11:00:00Z","body":""}
        ]"#,
        )
        .unwrap()
    }

    #[test]
    fn rows_return_the_pr_number() {
        let items = items(&prs());
        assert_eq!(items[0].value(), "7");
        assert_eq!(items[1].value(), "123");
    }

    #[test]
    fn rows_show_title_author_and_branch() {
        let label = &items(&prs())[0].label;
        assert!(label.contains("#7"));
        assert!(label.contains("Add branch picker"));
        assert!(label.contains("@joakimen"));
        assert!(label.contains("[feat/branches]"), "{label}");
    }

    #[test]
    fn drafts_are_tinted_differently() {
        let items = items(&prs());
        assert_ne!(items[0].color, items[1].color);
    }

    /// The preview must be ready-made text: a command here would mean a `gh`
    /// network call for every row the user scrolls past.
    #[test]
    fn preview_is_rendered_in_memory() {
        let Preview::Text(text) = preview(&prs()[0]) else {
            panic!("a PR preview must not spawn a command");
        };
        assert!(text.contains("#7"), "{text}");
        assert!(text.contains("Add branch picker"), "{text}");
        assert!(text.contains("@joakimen"), "{text}");
        assert!(text.contains("[feat/branches]"), "{text}");
        assert!(text.contains("Adds a picker"), "the body is the preview");
    }

    #[test]
    fn preview_handles_a_missing_description() {
        let Preview::Text(text) = preview(&prs()[1]) else {
            panic!("expected text");
        };
        assert!(text.contains("(no description)"), "{text}");
    }

    /// GitHub stores CRLF; leaving it in litters the pane with stray carriage
    /// returns.
    #[test]
    fn preview_normalises_line_endings() {
        let Preview::Text(text) = preview(&prs()[0]) else {
            panic!("expected text");
        };
        assert!(!text.contains('\r'), "CRLF leaked into the preview");
    }

    /// Numbers are padded to a common width so titles start in one column.
    #[test]
    fn number_column_aligns() {
        let items = items(&prs());
        assert_eq!(
            items[0].label.find("Add branch picker"),
            items[1].label.find("WIP"),
        );
    }
}
