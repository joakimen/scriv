//! `scriv pr` — list, pick, check out, open, and merge GitHub pull requests.
//!
//! Everything here goes through the `gh` CLI (see [`crate::gh`]), so scriv
//! inherits whatever authentication `gh auth login` set up — including SSO and
//! GitHub Enterprise hosts — and stores no credentials of its own.

use anyhow::{Result, bail};

use crate::Ctx;
use crate::gh::{self, MergeMethod, PullRequest};
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

/// Which status glyph columns this particular set of pull requests earns.
///
/// A column is a fixed [`gh::GLYPH_WIDTH`] — every glyph is that wide, so
/// nothing has to be measured per row — and appears only when some pull request
/// in the set has something to put in it. A repository with no CI gets no check
/// column; a list where everything merges cleanly gets no conflict column. The
/// alternative is columns of blank space on every row of every repository, to
/// report the absence of news.
#[derive(Clone, Copy, Default)]
struct StatusColumns {
    checks: bool,
    merge: bool,
}

impl StatusColumns {
    fn of(prs: &[PullRequest]) -> Self {
        Self {
            checks: prs.iter().any(|pr| !pr.checks().is_empty()),
            merge: prs
                .iter()
                .any(|pr| !pr.mergeable().glyph().trim().is_empty()),
        }
    }

    /// One pull request's glyphs, followed by the usual two-space gap, ready to
    /// splice in front of the title. Empty when the set earned no columns at
    /// all, so the gap does not appear either.
    ///
    /// `back` is the colour of the row these cells sit in: given one, each
    /// glyph is painted green/red/yellow and hands the row's colour back, so a
    /// listing shows a green `✓` without losing the tint of the rest of the
    /// line. Given `None` the glyphs are bare, which is what the picker needs —
    /// skim renders a row through ratatui and does not interpret ANSI in it, so
    /// an escape in a label would show up as literal rubbish.
    fn cells(self, pr: &PullRequest, back: Option<u8>) -> String {
        let paint = |glyph: &str, color: u8| match back {
            Some(back) => term::paint_within(glyph, color, back, true),
            None => glyph.to_string(),
        };

        let mut cells = Vec::new();
        if self.checks {
            let checks = pr.checks();
            cells.push(paint(checks.glyph(), checks.color()));
        }
        if self.merge {
            let mergeable = pr.mergeable();
            cells.push(paint(mergeable.glyph(), mergeable.color()));
        }
        if cells.is_empty() {
            return String::new();
        }
        format!("{}  ", cells.join(" "))
    }
}

/// `scriv pr ls` — print one pull request per line.
///
/// The check and conflict glyphs are here in the plain listing, not behind
/// `--status`: whether a pull request is green is the thing you scan a list of
/// them for, and a column or two is a price the bare form can pay. `--status`
/// adds what is genuinely extra — the open/draft/merged/closed tag, the source
/// branch, and the last-updated date.
///
/// The row is tinted by state and each glyph carries its own colour on top —
/// green `✓`, red `✗`. Colour is dropped when stdout is not a terminal, and the
/// glyphs are shapes rather than colours, so a piped or `NO_COLOR` listing
/// still says everything a coloured one does.
pub fn ls(ctx: &Ctx, state: &str, limit: usize, status: bool) -> Result<()> {
    let prs = collect(ctx, state, limit)?;
    let color = term::stdout_color();
    let width = number_width(&prs);
    let columns = StatusColumns::of(&prs);

    for pr in &prs {
        let cells = columns.cells(pr, color.then(|| pr.color()));
        let row = if status {
            format!(
                "#{number:<width$}  {tag:<6}  {cells}{title}  @{author}  [{head}]  {updated}",
                number = pr.number,
                tag = pr.tag(),
                title = pr.title,
                author = pr.author_login(),
                head = pr.head_ref_name,
                updated = pr.updated_date(),
            )
        } else {
            format!(
                "#{number:<width$}  {cells}{title}  @{author}",
                number = pr.number,
                title = pr.title,
                author = pr.author_login(),
            )
        };
        println!("{}", term::paint(&row, pr.color(), color));
    }
    Ok(())
}

/// How many failing or running checks the preview names before it stops.
/// A matrix build can report a hundred; the pane is not a check log.
const PREVIEW_CHECKS: usize = 10;

/// The checks-and-mergeability block, or nothing when there is neither.
///
/// Green checks are counted, not listed — thirty passing jobs say nothing that
/// `pass` does not. What is failing or still running gets named, since that is
/// the thing you open a pull request list to find out.
fn check_lines(pr: &PullRequest) -> String {
    let checks = pr.checks();
    let mergeable = pr.mergeable();
    let mut parts = Vec::new();
    if !checks.is_empty() {
        parts.push(format!(
            "checks {}",
            term::paint(&checks.summary(), checks.color(), true)
        ));
    }
    if !mergeable.tag().is_empty() {
        parts.push(format!(
            "mergeable {}",
            term::paint(mergeable.tag(), mergeable.color(), true)
        ));
    }
    if parts.is_empty() {
        return String::new();
    }

    let mut out = parts.join("   ");
    out.push('\n');
    let failing = pr.failing_checks();
    for check in failing.iter().take(PREVIEW_CHECKS) {
        // One glyph, one column, so the names line up with no padding to get
        // wrong. A plain `paint` is right here: the line ends after the name,
        // so resetting to the terminal default costs nothing.
        let result = check.result();
        out.push_str(&format!(
            "  {glyph}  {name}\n",
            glyph = term::paint(result.glyph(), result.color(), true),
            name = check.label(),
        ));
    }
    if failing.len() > PREVIEW_CHECKS {
        out.push_str(&format!(
            "  … and {} more\n",
            failing.len() - PREVIEW_CHECKS
        ));
    }
    out
}

/// The preview for a pull request: its heading, check rollup, and description,
/// rendered from data already in memory.
///
/// Deliberately *not* `gh pr view`. That would be prettier — it renders
/// markdown — but it is a network round trip (~2s here) per highlighted row,
/// spawned again on every move through the list. skim runs preview commands on
/// a background thread and does not kill non-PTY children, so scrolling would
/// leave a queue of `gh` processes racing to finish. The same goes for
/// `gh pr checks`: the rollup below is the one `gh pr list` already returned.
fn preview(pr: &PullRequest) -> Preview {
    let mut out = term::paint(
        &format!("#{number}  {title}", number = pr.number, title = pr.title),
        pr.color(),
        true,
    );
    out.push('\n');
    out.push_str(&format!(
        "{tag}  @{author}  [{head}]  updated {updated}\n",
        tag = pr.tag(),
        author = pr.author_login(),
        head = pr.head_ref_name,
        updated = pr.updated_date(),
    ));
    out.push_str(&check_lines(pr));
    out.push('\n');

    let body = pr.body.trim();
    if body.is_empty() {
        out.push_str("(no description)");
    } else {
        // GitHub stores CRLF; the pane renders the stray CRs as artefacts.
        out.push_str(&body.replace("\r\n", "\n"));
    }
    Preview::Text(out)
}

/// What tints a picker row.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tint {
    /// By pull request state — green open, grey draft, magenta merged, red
    /// closed. The right answer when you are choosing *which* pull request.
    State,
    /// By readiness to merge — green ready, yellow waiting on checks, red
    /// blocked, grey not a candidate. The right answer when the question is
    /// whether it *can* be merged, where a state colouring would paint a list
    /// of open pull requests one uniform green.
    Readiness,
}

impl Tint {
    fn color(self, pr: &PullRequest) -> u8 {
        match self {
            Self::State => pr.color(),
            Self::Readiness => pr.readiness().color(),
        }
    }
}

/// Build picker rows. The check and conflict glyphs and the source branch are
/// part of the label, so a glance down the list says what is green without
/// reading a word of it.
///
/// The glyphs go in bare: skim renders a row through ratatui and tints it from
/// [`PickItem::color`], with no ANSI parsing along the way, so an escape in the
/// label would appear as literal rubbish. The shapes carry the meaning, and the
/// row's own tint carries the colour.
fn items(prs: &[PullRequest], tint: Tint) -> Vec<PickItem> {
    let width = number_width(prs);
    let columns = StatusColumns::of(prs);
    prs.iter()
        .map(|pr| {
            let label = format!(
                "#{number:<width$}  {cells}{title}  @{author}  [{head}]",
                number = pr.number,
                cells = columns.cells(pr, None),
                title = pr.title,
                author = pr.author_login(),
                head = pr.head_ref_name,
            );
            PickItem::new(label, pr.number.to_string())
                .color(tint.color(pr))
                .preview(preview(pr))
        })
        .collect()
}

/// Fuzzy-select one pull request and return its number, re-asking `gh` and
/// reopening on [`REFRESH_KEY`](pick::REFRESH_KEY).
///
/// A pull request list is stale the moment it is drawn — a check finishes, a
/// review lands, someone merges — and a picker over it is exactly where you
/// notice. The reload is the same `gh pr list` the command opened with, run
/// between pickers rather than inside one: `gh` is a network round trip, and
/// skim's preview thread is no place for it.
fn select(ctx: &Ctx, state: &str, limit: usize, prompt: &str, tint: Tint) -> Result<u64> {
    let mut prs = collect(ctx, state, limit)?;
    let mut query = String::new();
    loop {
        match pick::pick_one_refreshable(items(&prs, tint), prompt, &query, &ctx.config.picker)? {
            pick::Picked::Chosen(choice) => {
                return choice
                    .parse()
                    .map_err(|_| anyhow::anyhow!("unexpected picker result: {choice}"));
            }
            pick::Picked::Refresh { query: typed } => {
                query = typed;
                ctx.log.info("refreshing pull requests");
                prs = collect(ctx, state, limit)?;
            }
        }
    }
}

/// `scriv pr pick` — fuzzy-select a pull request and print its number, so it
/// composes with `gh`: `gh pr view (scriv pr pick)`.
pub fn pick(ctx: &Ctx, state: &str, limit: usize) -> Result<()> {
    let number = select(ctx, state, limit, "Pick a pull request", Tint::State)?;
    println!("{number}");
    Ok(())
}

/// Resolve the pull request to act on: the number given, or one the user picks.
fn resolve(
    ctx: &Ctx,
    number: Option<u64>,
    state: &str,
    limit: usize,
    prompt: &str,
    tint: Tint,
) -> Result<u64> {
    match number {
        Some(number) => Ok(number),
        None => select(ctx, state, limit, prompt, tint),
    }
}

/// `scriv pr checkout [number]` — check out a pull request's branch, picking
/// one when no number is given. The checkout itself is `gh pr checkout`, which
/// handles fork PRs and sets the upstream.
pub fn checkout(ctx: &Ctx, number: Option<u64>, state: &str, limit: usize) -> Result<()> {
    let number = resolve(
        ctx,
        number,
        state,
        limit,
        "Check out a pull request",
        Tint::State,
    )?;
    gh::checkout(number)
}

/// `scriv pr open [number]` — open a pull request in the browser, picking one
/// when no number is given.
///
/// The picker is the point: it is the step between "one of these forty" and a
/// URL, and the preview means you can read the description before deciding
/// which one to open.
pub fn open(ctx: &Ctx, number: Option<u64>, state: &str, limit: usize) -> Result<()> {
    let number = resolve(
        ctx,
        number,
        state,
        limit,
        "Open a pull request",
        Tint::State,
    )?;
    gh::view_web(number)
}

/// `scriv pr merge [number]` — merge a pull request, picking one when no number
/// is given.
///
/// This is the one picker tinted by [`Tint::Readiness`] rather than by state.
/// It defaults to open pull requests, where a state colouring would make every
/// row the same green and say nothing; what you want to see at the moment you
/// choose one to merge is which of them is actually ready, so the whole row
/// goes green, yellow or red on that. Selecting a row takes a deliberate
/// `enter`, which is the confirmation step; from there this is `gh pr merge`,
/// including its interactive prompt for the merge method when none of
/// `--merge`/`--squash`/`--rebase` is given.
pub fn merge(
    ctx: &Ctx,
    number: Option<u64>,
    state: &str,
    limit: usize,
    method: Option<MergeMethod>,
    delete_branch: bool,
    auto: bool,
) -> Result<()> {
    let number = resolve(
        ctx,
        number,
        state,
        limit,
        "Merge a pull request",
        Tint::Readiness,
    )?;
    gh::merge(number, method, delete_branch, auto)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prs() -> Vec<PullRequest> {
        gh::parse_prs(
            r#"[
            {"number":7,"title":"Add branch picker","author":{"login":"joakimen"},
             "headRefName":"feat/branches","isDraft":false,"state":"OPEN",
             "updatedAt":"2026-07-27T09:12:33Z","mergeable":"MERGEABLE",
             "statusCheckRollup":[
                {"name":"build","workflowName":"ci","status":"COMPLETED","conclusion":"SUCCESS"}
             ],
             "body":"Adds a picker.\r\n\r\nSecond paragraph."},
            {"number":123,"title":"WIP","author":{"login":"someone"},
             "headRefName":"wip","isDraft":true,"state":"OPEN",
             "updatedAt":"2026-07-20T11:00:00Z","mergeable":"CONFLICTING",
             "statusCheckRollup":[
                {"name":"build","workflowName":"ci","status":"COMPLETED","conclusion":"FAILURE"},
                {"name":"test","workflowName":"ci","status":"IN_PROGRESS","conclusion":""}
             ],
             "body":""}
        ]"#,
        )
        .unwrap()
    }

    /// A repository with no CI, where GitHub has also not reported
    /// mergeability — the case the status columns have to disappear for.
    fn bare_prs() -> Vec<PullRequest> {
        gh::parse_prs(
            r#"[
            {"number":1,"title":"Only one","author":{"login":"ada"},
             "headRefName":"only","isDraft":false,"state":"OPEN",
             "updatedAt":"2026-07-27T09:12:33Z","body":""}
        ]"#,
        )
        .unwrap()
    }

    #[test]
    fn rows_return_the_pr_number() {
        let items = items(&prs(), Tint::State);
        assert_eq!(items[0].value(), "7");
        assert_eq!(items[1].value(), "123");
    }

    #[test]
    fn rows_show_title_author_and_branch() {
        let label = &items(&prs(), Tint::State)[0].label;
        assert!(label.contains("#7"));
        assert!(label.contains("Add branch picker"));
        assert!(label.contains("@joakimen"));
        assert!(label.contains("[feat/branches]"), "{label}");
    }

    #[test]
    fn drafts_are_tinted_differently() {
        let items = items(&prs(), Tint::State);
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

    /// The column a title starts in, as the terminal counts it.
    ///
    /// Not `str::find`, which is a byte offset: the status glyphs are three and
    /// four bytes for the same two columns, so byte offsets differ between rows
    /// that line up perfectly on screen — and match on rows that do not.
    fn title_column(label: &str, title: &str) -> usize {
        use unicode_width::UnicodeWidthStr;
        let start = label.find(title).expect("title in label");
        label[..start].width()
    }

    /// Numbers are padded to a common width so titles start in one column.
    #[test]
    fn number_column_aligns() {
        let items = items(&prs(), Tint::State);
        assert_eq!(
            title_column(&items[0].label, "Add branch picker"),
            title_column(&items[1].label, "WIP"),
        );
    }

    /// The check rollup is in the label, not only the preview, so a glance down
    /// the picker tells you what is green without reading a word.
    #[test]
    fn rows_carry_the_check_rollup() {
        let items = items(&prs(), Tint::State);
        assert!(items[0].label.contains('✓'), "{}", items[0].label);
        assert!(items[1].label.contains('✗'), "{}", items[1].label);
        assert!(items[1].label.contains('⊘'), "{}", items[1].label);
    }

    /// A repository with no CI must not pay a column of blank space on every
    /// row for checks it does not have.
    #[test]
    fn status_columns_vanish_when_there_is_nothing_to_show() {
        let columns = StatusColumns::of(&bare_prs());
        assert_eq!(columns.cells(&bare_prs()[0], None), "");
        let label = &items(&bare_prs(), Tint::State)[0].label;
        assert_eq!(label, "#1  Only one  @ada  [only]");
    }

    /// Only the columns that are earned appear: these pull requests have checks
    /// and one conflict, so both columns exist and the clean one is a blank of
    /// the same width — never a shorter row.
    #[test]
    fn status_columns_align() {
        use unicode_width::UnicodeWidthStr;
        let items = items(&prs(), Tint::State);
        assert_eq!(
            title_column(&items[0].label, "Add branch picker"),
            title_column(&items[1].label, "WIP"),
        );
        let columns = StatusColumns::of(&prs());
        let widths: Vec<usize> = prs()
            .iter()
            .map(|pr| columns.cells(pr, None).width())
            .collect();
        // Two glyph columns, the space between them, and the gap before the
        // title.
        assert_eq!(widths, [gh::GLYPH_WIDTH * 2 + 3; 2]);
    }

    /// The conflict column is only paid for when something actually conflicts.
    #[test]
    fn the_conflict_column_appears_only_when_one_exists() {
        use unicode_width::UnicodeWidthStr;
        let clean = &prs()[..1];
        let columns = StatusColumns::of(clean);
        assert_eq!(columns.cells(&clean[0], None).width(), gh::GLYPH_WIDTH + 2);
    }

    /// `ls` shows the glyphs without `--status`: whether a pull request is
    /// green is the thing the plain listing is read for.
    #[test]
    fn the_bare_listing_carries_the_glyphs() {
        // `ls` builds its bare row from the same cells the picker uses.
        let columns = StatusColumns::of(&prs());
        assert!(columns.cells(&prs()[0], None).contains('✓'));
        assert!(columns.cells(&prs()[1], None).contains('✗'));
    }

    /// The merge picker is tinted by readiness, not state: a list of open pull
    /// requests is one shade of green under a state colouring, exactly when the
    /// colour would be most useful.
    #[test]
    fn the_merge_picker_tints_by_readiness() {
        let prs = prs();
        let state = items(&prs, Tint::State);
        let readiness = items(&prs, Tint::Readiness);
        // #7 is open with green checks; #123 is a draft with a failing check.
        assert_eq!(state[0].color, Some(prs[0].color()));
        assert_eq!(readiness[0].color, Some(gh::Readiness::Ready.color()));
        assert_eq!(
            readiness[1].color,
            Some(gh::Readiness::Unavailable.color()),
            "a draft is not a merge candidate",
        );
    }

    /// The rollup belongs in the preview too: the list says `fail`, the pane
    /// says which job.
    #[test]
    fn preview_names_the_failing_checks() {
        let Preview::Text(text) = preview(&prs()[1]) else {
            panic!("expected text");
        };
        assert!(text.contains("1 failed, 1 pending"), "{text}");
        assert!(text.contains("build (ci)"), "{text}");
        assert!(text.contains("test (ci)"), "{text}");
        assert!(text.contains("conflict"), "{text}");
    }

    /// Each named check is prefixed by its own glyph, which is both the colour
    /// and the column width — so the names line up with no padding to get
    /// wrong, and the failing one is findable without reading the list.
    #[test]
    fn preview_marks_each_named_check_with_its_glyph() {
        let text = strip_ansi(&check_lines(&prs()[1]));
        let rows: Vec<&str> = text.lines().filter(|line| line.starts_with("  ")).collect();
        assert_eq!(rows, ["  ✗  build (ci)", "  ⧗  test (ci)"]);
    }

    /// Drop ANSI colour sequences, so a test can measure what the pane shows.
    fn strip_ansi(text: &str) -> String {
        let mut out = String::new();
        let mut chars = text.chars();
        while let Some(c) = chars.next() {
            if c != '\x1b' {
                out.push(c);
                continue;
            }
            for c in chars.by_ref() {
                if c == 'm' {
                    break;
                }
            }
        }
        out
    }

    /// Thirty green jobs are worth a count, not thirty lines.
    #[test]
    fn preview_counts_passing_checks_without_listing_them() {
        let Preview::Text(text) = preview(&prs()[0]) else {
            panic!("expected text");
        };
        assert!(text.contains("1 passed"), "{text}");
        assert!(!text.contains("build (ci)"), "a passing check was listed");
    }

    /// Without CI or a mergeability verdict there is no status line to show,
    /// and the preview must not leave a stray heading behind.
    #[test]
    fn preview_omits_the_status_line_when_empty() {
        assert_eq!(check_lines(&bare_prs()[0]), "");
        let Preview::Text(text) = preview(&bare_prs()[0]) else {
            panic!("expected text");
        };
        assert!(!text.contains("checks"), "{text}");
        assert!(!text.contains("mergeable"), "{text}");
    }
}
