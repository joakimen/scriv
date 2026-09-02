//! `scriv repo` — list and select the Git repositories found under the
//! configured search paths.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context, Result, bail};

use crate::config::{RepoDisplay, label_colors};
use crate::gh::{self, Repo};
use crate::path::{display_path, expand_home_dir, relative_label};
use crate::repo::FoundRepo;
use crate::select::{Choice, SelectItem, Tint};
use crate::{Ctx, git, repo, select, term};

/// What to say when there is nowhere to look for repositories. The two ways to
/// arrive here need opposite advice, and telling someone who has just written a
/// config to write one is how a first run stalls: `config init` refuses to
/// overwrite the file it is being recommended for.
fn no_root_message(config_path: &Path, config_exists: bool) -> String {
    if config_exists {
        format!(
            "no `root` configured in {} — set `[repo] root` to the directory \
             holding your <owner>/<repo> checkouts",
            config_path.display()
        )
    } else {
        format!(
            "no configuration at {} — run `scriv config init` to write a starter one",
            config_path.display()
        )
    }
}

/// Discover repositories, label-tagged and sorted by path.
fn discover(ctx: &Ctx) -> Result<Vec<FoundRepo>> {
    if ctx.config.repo.root.is_none() && ctx.config.repo.extra.is_empty() {
        anyhow::bail!(no_root_message(&ctx.config_path, ctx.config_path.exists()));
    }
    ctx.log.info(&format!(
        "settings: ignore = {}",
        ctx.config.repo.ignore.join(", ")
    ));

    let mut repos = repo::find_all_repos(&ctx.config, ctx.home(), &ctx.log)
        .with_context(|| format!("using configuration {}", ctx.config_path.display()))?;
    repos.sort_by(|a, b| a.path.as_os_str().cmp(b.path.as_os_str()));

    if repos.is_empty() {
        anyhow::bail!(
            "no repositories found under the configured paths (check depths in {})",
            ctx.config_path.display()
        );
    }
    Ok(repos)
}

/// `scriv repo ls` — print every discovered repository, one per line,
/// home-collapsed unless `absolute`.
pub fn ls(ctx: &Ctx, absolute: bool) -> Result<()> {
    let repos = discover(ctx)?;
    ctx.log
        .info(&format!("returning {} repositories", repos.len()));
    let mut out = term::Listing::stdout();
    for repo in &repos {
        let path = repo.path.to_string_lossy();
        if !out.line(&display_path(&path, ctx.home_str(), absolute))? {
            break;
        }
    }
    out.finish()?;
    Ok(())
}

/// The selector rows for the discovered repositories: each prefixed with its
/// label and coloured by it, paths rendered per `repo.display`. Every row's
/// value is the absolute path, so a caller never re-expands `~`.
fn repo_rows(ctx: &Ctx, repos: &[FoundRepo]) -> Vec<SelectItem> {
    let colors = label_colors(&ctx.config.repo.labels);

    // Character count, not bytes: `{:<width$}` pads by characters.
    let width = repos
        .iter()
        .map(|r| r.label.chars().count())
        .max()
        .unwrap_or(0);

    let mode = ctx.config.repo.display;
    repos
        .iter()
        .map(|repo| {
            let abs = repo.path.to_string_lossy().into_owned();
            let shown = match mode {
                RepoDisplay::Relative => relative_label(&repo.path, &repo.root),
                RepoDisplay::Tilde => display_path(&abs, ctx.home_str(), false),
                RepoDisplay::Full => abs.clone(),
            };
            // The label is a column, not something anyone searches by: in the
            // prefix it is drawn and not matched, so a query only ever reaches
            // the repository itself.
            let label = format!("{label:<width$}  ", label = repo.label);
            let color = colors.get(repo.label.as_str()).copied();
            let tints = color.map_or_else(Vec::new, |color| {
                vec![Tint {
                    range: 0..label.chars().count(),
                    color,
                }]
            });

            let item = SelectItem::new(shown, abs.clone())
                .prefix(label, tints)
                .preview(select::checkout_preview(&abs));
            match color {
                Some(color) => item.color(color),
                None => item,
            }
        })
        .collect()
}

/// Open the highlighted repository's GitHub page instead of returning it. f1 is
/// what does that from the prompt in fish, and means the same thing here — so
/// the answer to "which one" can be given once whichever question was asked.
const OPEN: select::Action = select::Action::new("f1", "on GitHub");

/// `scriv repo sel` — fuzzy-select one repository and print its absolute path.
///
/// The path is printed absolute so a shell shim can `cd` to it directly.
///
/// [`OPEN`] prints nothing, which the fish `cd` wrapper already treats as
/// nothing to do — the same as cancelling.
pub fn sel(ctx: &Ctx) -> Result<()> {
    let repos = discover(ctx)?;
    let (rows, now) = ctx.by_recency(repo_rows(ctx, &repos), |row| row.value());
    let chosen =
        select::select_one_acting(rows, "Select a repository", &ctx.config.selector, &[OPEN])?;
    // Recorded either way: which repository was wanted is the answer, and what
    // was then done with it does not make it a worse guess next time.
    ctx.remember(&chosen.value, now);
    if chosen.action.is_some() {
        return gh::view_repo_web(Path::new(&chosen.value));
    }
    println!("{}", chosen.value);
    Ok(())
}

/// Which repository `repo open` acts on.
#[derive(Debug, PartialEq, Eq)]
enum Target {
    /// The repository the shell is standing in.
    Here(PathBuf),
    /// Whichever one the user selects.
    Select,
}

/// Decide what `repo open` opens. Standing in a repository already says which
/// one is meant, so it wins over the selector; `--select` overrides that.
fn target(root: Option<PathBuf>, force_select: bool) -> Target {
    match root {
        Some(root) if !force_select => Target::Here(root),
        _ => Target::Select,
    }
}

/// `scriv repo open` — open a repository's GitHub page in the browser. Inside a
/// repository that is this one; anywhere else, or with `--select`, it selects
/// from every repository scriv found.
pub fn open(ctx: &Ctx, force_select: bool) -> Result<()> {
    if let Target::Here(root) = target(git::repo_root(), force_select) {
        ctx.log
            .info(&format!("opening the repository at {}", root.display()));
        return gh::view_repo_web(&root);
    }

    let repos = discover(ctx)?;
    let (rows, now) = ctx.by_recency(repo_rows(ctx, &repos), |row| row.value());
    let choice = select::select_one(rows, "Open a repository on GitHub", &ctx.config.selector)?;
    ctx.remember(&choice, now);
    gh::view_repo_web(Path::new(&choice))
}

// --- clone ------------------------------------------------------------------

/// How many clones run at once. Network-bound, so well above the core count;
/// the ceiling is what a remote accepts from one user.
const CLONE_CONCURRENCY: usize = 8;

/// The mark on a repository already on disk, and its colour: green, the one
/// hue that reads as "done" without being read as a warning. It is the whole
/// signal — the row beneath it is drawn like any other, since a dimmed row says
/// "not worth looking at" of a repository that is merely already here.
const PRESENT_MARK: &str = "✓";
const PRESENT_COLOR: u8 = 2;

/// The configured root, expanded — the one directory clones are written to.
fn clone_root(ctx: &Ctx) -> Result<PathBuf> {
    let root = ctx.config.repo.root.as_deref().ok_or_else(|| {
        anyhow::anyhow!(
            "{} — `repo clone` writes to <root>/<owner>/<repo>",
            no_root_message(&ctx.config_path, ctx.config_path.exists())
        )
    })?;
    Ok(expand_home_dir(root, ctx.home()))
}

/// Where `owner/repo` belongs on disk — the inverse of the discovery walk, so a
/// clone lands exactly where `repo sel` looks for it.
fn destination(root: &Path, owner: &str, name: &str) -> PathBuf {
    root.join(owner).join(name)
}

/// Owners worth offering, most useful first: those named in the config, then
/// any already on disk under the root, then the user's own login and orgs —
/// the last being the only source that works on a machine with nothing cloned.
fn owner_candidates(ctx: &Ctx, root: &Path) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    let mut push = |owner: &str, out: &mut Vec<String>| {
        let key = owner.to_ascii_lowercase();
        if !owner.is_empty() && seen.insert(key) {
            out.push(owner.to_string());
        }
    };

    for owner in ctx.config.repo.known_owners() {
        push(owner, &mut out);
    }
    if let Ok(entries) = std::fs::read_dir(root) {
        let mut local: Vec<String> = entries
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|name| !name.starts_with('.'))
            .collect();
        local.sort_by_key(|n| n.to_ascii_lowercase());
        for owner in local {
            push(&owner, &mut out);
        }
    }
    // A round trip to GitHub, and the owner selector cannot open until it is
    // back.
    let asked = {
        let _spinner = term::spinner("asking GitHub which owners you have", ctx.color());
        gh::owners()
    };
    match asked {
        Ok(owners) => {
            for owner in owners {
                push(&owner, &mut out);
            }
        }
        Err(err) => ctx.log.warn(&format!("could not ask gh for owners: {err}")),
    }
    out
}

/// Fuzzy-select an owner, accepting one that was typed but not listed.
fn select_owner(ctx: &Ctx, root: &Path) -> Result<String> {
    let candidates = owner_candidates(ctx, root);
    ctx.log
        .info(&format!("{} owner candidates", candidates.len()));

    let colors = label_colors(&ctx.config.repo.labels);
    let items: Vec<SelectItem> = candidates
        .iter()
        .map(|owner| {
            let label = ctx.config.repo.label_of(owner);
            let row = match label {
                Some(l) => format!("{owner}  ({l})"),
                None => owner.clone(),
            };
            let item = SelectItem::new(row, owner.clone());
            match label.and_then(|l| colors.get(l).copied()) {
                Some(color) => item.color(color),
                None => item,
            }
        })
        .collect();

    match select::select_one_or_query(items, "Owner (type any GitHub owner)", &ctx.config.selector)?
    {
        Choice::Item(owner) => Ok(owner),
        Choice::Query(typed) => {
            ctx.log.info(&format!("owner typed, not listed: {typed}"));
            Ok(typed)
        }
    }
}

/// How wide each column of the clone selector is, measured over the whole list
/// so the columns line up.
struct Widths {
    name: usize,
    tags: usize,
    pushed: usize,
}

impl Widths {
    fn of<'a>(repos: impl Iterator<Item = &'a Repo> + Clone) -> Self {
        // Character counts, not bytes: a column is padded by characters.
        let widest = |f: fn(&Repo) -> usize| repos.clone().map(f).max().unwrap_or(0);
        Self {
            name: widest(|r| r.name().chars().count()),
            tags: widest(|r| tag_column(r).chars().count()),
            pushed: widest(|r| r.pushed_date().chars().count()),
        }
    }
}

/// The tags column: what makes this repository unusual, in one string.
fn tag_column(repo: &Repo) -> String {
    repo.tags().join(" ")
}

/// Append `text` and pad it out to `width` characters.
fn push_column(row: &mut String, text: &str, width: usize) {
    row.push_str(text);
    for _ in text.chars().count()..width {
        row.push(' ');
    }
}

/// One row of the clone selector, split into what a query may match and what it
/// may not.
///
/// The three are built together because a tint is a character range into the
/// part it belongs to, and counting those ranges out a second time is how they
/// drift.
///
/// Nothing tints the row as a whole. A line drawn in one colour reads as a
/// statement about the repository, and neither "already on disk" nor "private"
/// is one — the first is about this machine and the second about one column, so
/// each colours only the thing it is true of.
struct CloneRow {
    /// The tick for a repository already on disk, ahead of the name.
    mark: String,
    mark_tints: Vec<Tint>,
    /// The only part a query is matched against.
    name: String,
    /// The tags, the date it was last pushed to, and the description.
    rest: String,
    rest_tints: Vec<Tint>,
}

fn clone_row(repo: &Repo, present: bool, widths: &Widths) -> CloneRow {
    let mut mark = String::from(if present { PRESENT_MARK } else { " " });
    let mark_tints = if present {
        vec![Tint {
            range: 0..mark.chars().count(),
            color: PRESENT_COLOR,
        }]
    } else {
        Vec::new()
    };
    mark.push(' ');

    let mut name = String::new();
    push_column(&mut name, repo.name(), widths.name);

    let mut rest = String::from("  ");
    let mut rest_tints = Vec::new();
    let visibility = repo.visibility();
    let tags_at = rest.chars().count();
    for (index, tag) in repo.tags().iter().enumerate() {
        if index > 0 {
            rest.push(' ');
        }
        let at = rest.chars().count();
        rest.push_str(tag);
        if let Some(color) = visibility.color()
            && *tag == visibility.tag()
        {
            rest_tints.push(Tint {
                range: at..rest.chars().count(),
                color,
            });
        }
    }
    for _ in (rest.chars().count() - tags_at)..widths.tags {
        rest.push(' ');
    }
    rest.push_str("  ");

    push_column(&mut rest, repo.pushed_date(), widths.pushed);
    rest.push_str("  ");
    rest.push_str(repo.description().trim());

    CloneRow {
        mark,
        mark_tints,
        name,
        rest: rest.trim_end().to_string(),
        rest_tints,
    }
}

/// Which of an owner's repositories the clone selector is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CloneView {
    /// Everything the owner has, cloned or not. What it opens on: a repository
    /// missing from the list would read as "this org does not have that repo".
    All,
    /// Only the ones not already under the root — what there is left to clone.
    Missing,
}

impl CloneView {
    /// The view at `index` in [`CLONE_VIEWS`].
    fn of(index: usize) -> Self {
        match index {
            1 => Self::Missing,
            _ => Self::All,
        }
    }

    /// Whether a repository belongs in this view. `present` is whether it is
    /// already on disk.
    fn shows(self, present: bool) -> bool {
        match self {
            Self::All => true,
            Self::Missing => !present,
        }
    }
}

/// The keys the clone selector offers its views on, in the order the header
/// lists them. `ctrl-a` displaces skim's own beginning-of-line, which in a
/// one-line query is worth less than reaching the whole list again.
const CLONE_VIEWS: &[select::Mode] = &[
    select::Mode::new("ctrl-a", "all"),
    select::Mode::new("ctrl-t", "missing"),
];

/// Build the repository rows for `view`, marking the ones already on disk.
///
/// Only the name is matched against: the tags, the date and the description sit
/// in the row's suffix, so a query cannot find a repository by words nobody
/// searches by — and cannot scroll the name off the row to show where it hit.
///
/// No preview pane: everything the listing returned that is worth seeing is
/// already a column, and a pane that repeats the row is a pane nobody reads.
fn repo_items(repos: &[Repo], root: &Path, view: CloneView) -> Vec<SelectItem> {
    let shown: Vec<(&Repo, bool)> = repos
        .iter()
        .map(|repo| (repo, destination(root, repo.owner(), repo.name()).exists()))
        .filter(|(_, present)| view.shows(*present))
        .collect();

    let widths = Widths::of(shown.iter().map(|(repo, _)| *repo));
    shown
        .iter()
        .map(|(repo, present)| {
            let row = clone_row(repo, *present, &widths);
            SelectItem::new(row.name, repo.name_with_owner().to_string())
                .prefix(row.mark, row.mark_tints)
                .suffix(row.rest, row.rest_tints)
        })
        .collect()
}

/// Clone `repos` into `root`, [`CLONE_CONCURRENCY`] at a time, from a shared
/// queue rather than fixed batches — clone times vary by orders of magnitude.
/// Results are reported in request order, and one failure does not stop the
/// others.
fn clone_all(ctx: &Ctx, root: &Path, repos: &[String]) -> Result<usize> {
    let next = AtomicUsize::new(0);
    let done: Mutex<Vec<(usize, Result<PathBuf>)>> = Mutex::new(Vec::with_capacity(repos.len()));

    std::thread::scope(|scope| {
        for _ in 0..CLONE_CONCURRENCY.min(repos.len()) {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(slug) = repos.get(index) else { break };
                    let (owner, name) = slug.split_once('/').unwrap_or(("", slug.as_str()));
                    let dest = destination(root, owner, name);
                    let result = gh::clone(slug, &dest).map(|()| dest);
                    // A worker that panicked mid-clone poisons this; what was
                    // already collected is still worth reporting.
                    done.lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push((index, result));
                }
            });
        }
    });

    let mut results = done.into_inner().unwrap_or_else(|e| e.into_inner());
    results.sort_by_key(|(index, _)| *index);

    let mut failures = 0;
    for (index, result) in results {
        let slug = &repos[index];
        match result {
            Ok(dest) => println!("cloned {slug} -> {}", dest.display()),
            Err(err) => {
                failures += 1;
                eprintln!("error: {slug}: {err:#}");
            }
        }
    }
    ctx.log.info(&format!("{failures} clone(s) failed"));
    Ok(failures)
}

/// `scriv repo clone [owner | owner/repo]` — clone repositories from GitHub
/// into the configured root.
///
/// With no argument, select an owner (from the config, the root, and `gh`, with
/// anything typed accepted), then one or more of that owner's repositories.
/// `owner/repo` skips both selectors, `archived` widens the list to the
/// repositories GitHub has retired. Everything lands at
/// `<root>/<owner>/<repo>`, which is where discovery looks.
pub fn clone(ctx: &Ctx, target: Option<&str>, limit: usize, archived: bool) -> Result<()> {
    let root = clone_root(ctx)?;

    // `owner/repo` names a repository outright; there is nothing to choose.
    if let Some(slug) = target.filter(|t| t.contains('/')) {
        check_slug(slug)?;
        return finish(ctx, &root, vec![slug.to_string()]);
    }

    let owner = match target {
        Some(owner) => owner.to_string(),
        None => select_owner(ctx, &root)?,
    };
    // The owner selector accepts anything typed, so it is exactly as unchecked
    // as an argument and gets the same check.
    check_slug(&owner)?;

    let repos = {
        // The pages are fetched together and arrive in whatever order they
        // finish, so there is no honest count to report — only that the wait is
        // this owner's listing.
        let _spinner = term::spinner(
            &format!(
                "loading repositories for {}",
                term::bold(&owner, ctx.color())
            ),
            ctx.color(),
        );
        gh::list_repos(&owner, limit, archived)?
    };
    ctx.log
        .info(&format!("{} repositories for {owner}", repos.len()));
    if repos.is_empty() {
        bail!("{}", empty_message(&owner, archived));
    }
    if repos.len() == limit {
        eprintln!(
            "note: showing the first {limit} repositories for {owner}; pass --limit to raise it"
        );
    }

    let chosen = {
        let listed = repos.clone();
        let here = root.clone();
        select::select_many_viewing(
            repo_items(&repos, &root, CloneView::All),
            "Repositories to clone (tab to select several)",
            &ctx.config.selector,
            CLONE_VIEWS,
            Box::new(move |view| repo_items(&listed, &here, CloneView::of(view))),
        )?
    };
    if chosen.is_empty() {
        return Ok(());
    }
    finish(ctx, &root, chosen)
}

/// What to say when an owner has nothing to offer. An owner whose every
/// repository is archived looks exactly like a misspelt one, so the default
/// filter names itself rather than leaving the user to doubt the owner.
fn empty_message(owner: &str, archived: bool) -> String {
    if archived {
        format!("no repositories found for {owner}")
    } else {
        format!("no unarchived repositories found for {owner}; pass --archived to include those")
    }
}

/// Reject a name that is not one GitHub could have issued. A slug is both a
/// positional argument to `gh` and the `<owner>/<repo>` [`destination`] joins
/// onto the root, so a leading `-` or a `..` has to be refused here.
fn check_slug(slug: &str) -> Result<()> {
    if gh::valid_slug(slug) {
        return Ok(());
    }
    bail!(
        "`{slug}` is not a GitHub owner or owner/repo name — those are letters, \
         digits, `-`, `_` and `.`, and do not begin with `-`"
    )
}

/// Skip what is already on disk, clone the rest, and exit non-zero if any
/// clone failed.
fn finish(ctx: &Ctx, root: &Path, chosen: Vec<String>) -> Result<()> {
    let (present, missing): (Vec<String>, Vec<String>) = chosen.into_iter().partition(|slug| {
        let (owner, name) = slug.split_once('/').unwrap_or(("", slug.as_str()));
        destination(root, owner, name).exists()
    });

    for slug in &present {
        let (owner, name) = slug.split_once('/').unwrap_or(("", slug.as_str()));
        println!(
            "already present, skipping: {slug} ({})",
            destination(root, owner, name).display()
        );
    }
    if missing.is_empty() {
        return Ok(());
    }

    let failures = clone_all(ctx, root, &missing)?;
    if failures > 0 {
        bail!("{failures} of {} clones failed", missing.len());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Labels;

    fn repos() -> Vec<Repo> {
        gh::parse_repos(
            r#"[
            {"full_name":"acme/billing-api","description":"Meters usage",
             "visibility":"private","archived":false,"fork":false,
             "language":"Rust","pushed_at":"2026-07-27T09:12:33Z"},
            {"full_name":"acme/old-thing","description":null,
             "visibility":"public","archived":true,"fork":true,
             "language":null,"pushed_at":"2024-01-02T00:00:00Z"}
        ]"#,
        )
        .unwrap()
    }

    fn labels(names: &[&str]) -> Labels {
        names
            .iter()
            .map(|n| ((*n).to_string(), Vec::new()))
            .collect()
    }

    #[test]
    fn the_conventional_labels_keep_their_colours() {
        let (first, last) = (labels(&["work", "personal"]), labels(&["personal", "work"]));
        let listed_first = label_colors(&first);
        let listed_last = label_colors(&last);
        assert_eq!(listed_first.get("work"), Some(&6), "work is cyan");
        assert_eq!(listed_first.get("personal"), Some(&2), "personal is green");
        assert_eq!(
            listed_first, listed_last,
            "config order changed the colours"
        );
        // No entry for the absence of a label: those rows stay uncoloured.
        assert!(!listed_first.contains_key(crate::config::UNLABELLED));
    }

    #[test]
    fn other_labels_avoid_the_reserved_hues() {
        let configured = labels(&["client", "work", "oss", "personal"]);
        let colors = label_colors(&configured);
        assert_eq!(colors.get("work"), Some(&6));
        assert_eq!(colors.get("personal"), Some(&2));
        let mut assigned: Vec<u8> = colors.values().copied().collect();
        assigned.sort_unstable();
        assigned.dedup();
        assert_eq!(assigned.len(), 4, "two labels share a colour: {colors:?}");
    }

    #[test]
    fn open_acts_on_the_repository_you_are_in() {
        let here = PathBuf::from("/home/u/dev/github.com/acme/billing-api");
        assert_eq!(target(Some(here.clone()), false), Target::Here(here));
    }

    #[test]
    fn open_selects_outside_a_repository_or_on_request() {
        assert_eq!(target(None, false), Target::Select);
        assert_eq!(target(None, true), Target::Select);
        let here = PathBuf::from("/home/u/dev/github.com/acme/billing-api");
        assert_eq!(target(Some(here), true), Target::Select);
    }

    #[test]
    fn destination_matches_the_discovery_layout() {
        let root = Path::new("/home/u/dev/github.com");
        let dest = destination(root, "joakimen", "scriv");
        assert_eq!(dest, PathBuf::from("/home/u/dev/github.com/joakimen/scriv"));
        // The inverse holds: discovery reads that owner back off the path.
        assert_eq!(
            repo::owner_of(root, &dest).as_deref(),
            Some("joakimen"),
            "clone destination and discovery disagree",
        );
    }

    #[test]
    fn splits_owner_and_name() {
        let repos = repos();
        assert_eq!(repos[0].owner(), "acme");
        assert_eq!(repos[0].name(), "billing-api");
        assert_eq!(repos[0].language(), "Rust");
        assert_eq!(repos[0].pushed_date(), "2026-07-27");
    }

    #[test]
    fn tags_only_mark_the_unusual() {
        let repos = repos();
        assert_eq!(repos[0].tags(), vec!["private"]);
        assert_eq!(repos[1].tags(), vec!["archived", "fork"]);
    }

    /// The characters `range` covers, which is what the selector will paint.
    fn tinted(row: &str, tint: &Tint) -> String {
        row.chars()
            .skip(tint.range.start)
            .take(tint.range.end - tint.range.start)
            .collect::<String>()
    }

    fn tint_of(row: &str, tints: &[Tint], color: u8) -> Option<String> {
        tints
            .iter()
            .find(|t| t.color == color)
            .map(|t| tinted(row, t))
    }

    #[test]
    fn present_repositories_are_marked_not_hidden() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("acme/billing-api")).unwrap();

        let items = repo_items(&repos(), root, CloneView::All);
        assert_eq!(items.len(), 2, "a present repo was dropped from the list");
        let mark = |item: &SelectItem| item.prefix.clone().unwrap_or_default();
        assert!(
            mark(&items[0]).starts_with(PRESENT_MARK),
            "{:?}",
            mark(&items[0])
        );
        assert!(
            !mark(&items[1]).starts_with(PRESENT_MARK),
            "{:?}",
            mark(&items[1])
        );
        // The value is still the slug, so selecting it is well-defined.
        assert_eq!(items[0].value(), "acme/billing-api");
    }

    #[test]
    fn nothing_colours_a_whole_row() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("acme/billing-api")).unwrap();
        for item in repo_items(&repos(), root, CloneView::All) {
            assert_eq!(item.color, None, "{} is drawn in one colour", item.label);
        }
    }

    #[test]
    fn the_mark_on_an_existing_clone_is_green_and_only_the_mark() {
        let widths = Widths::of(repos().iter());
        let row = clone_row(&repos()[0], true, &widths);
        assert_eq!(
            tint_of(&row.mark, &row.mark_tints, PRESENT_COLOR).as_deref(),
            Some("✓")
        );
        assert_eq!(PRESENT_COLOR, 2, "the mark is not green");
    }

    #[test]
    fn an_uncloned_repository_carries_no_mark() {
        let widths = Widths::of(repos().iter());
        let row = clone_row(&repos()[0], false, &widths);
        assert!(!row.mark.contains(PRESENT_MARK), "{:?}", row.mark);
        assert!(row.mark_tints.is_empty());
    }

    #[test]
    fn only_the_visibility_word_takes_the_visibility_colour() {
        let widths = Widths::of(repos().iter());
        let row = clone_row(&repos()[0], false, &widths);
        let color = gh::Visibility::Private.color().unwrap();
        assert_eq!(
            tint_of(&row.rest, &row.rest_tints, color).as_deref(),
            Some("private")
        );
    }

    /// `archived` and `fork` share the tags column with the visibility word and
    /// say nothing about who can see the repository.
    #[test]
    fn the_other_tags_are_left_uncoloured() {
        let widths = Widths::of(repos().iter());
        let row = clone_row(&repos()[1], false, &widths);
        assert!(row.rest.contains("archived fork"), "{:?}", row.rest);
        assert!(
            row.rest_tints.is_empty(),
            "a public repository is left bare: {:?}",
            row.rest
        );
    }

    #[test]
    fn the_row_carries_the_last_push_date_before_the_description() {
        let widths = Widths::of(repos().iter());
        let row = clone_row(&repos()[0], false, &widths);
        let pushed = row.rest.find("2026-07-27").expect(&row.rest);
        let description = row.rest.find("Meters usage").expect(&row.rest);
        assert!(pushed < description, "{:?}", row.rest);
    }

    /// Every column is padded to the width of the whole list, so a row reads
    /// down the screen rather than only across.
    #[test]
    fn the_columns_line_up() {
        let widths = Widths::of(repos().iter());
        let rows: Vec<String> = repos()
            .iter()
            .map(|repo| clone_row(repo, false, &widths).rest)
            .collect();
        let at = |row: &str, needle: &str| row.find(needle).map(|i| row[..i].chars().count());
        assert_eq!(
            at(&rows[0], "2026-07-27"),
            at(&rows[1], "2024-01-02"),
            "{rows:?}",
        );
    }

    #[test]
    fn nothing_previews_a_repository() {
        let tmp = tempfile::TempDir::new().unwrap();
        for item in repo_items(&repos(), tmp.path(), CloneView::All) {
            assert!(item.preview.is_none(), "{} opens a pane", item.label);
        }
    }

    #[test]
    fn the_advice_for_a_missing_root_depends_on_there_being_a_config() {
        let path = Path::new("/home/u/.config/scriv/config.toml");

        let written = no_root_message(path, true);
        assert!(written.contains("[repo] root"), "{written}");
        assert!(
            !written.contains("config init"),
            "advises a command that refuses to run: {written}"
        );

        let absent = no_root_message(path, false);
        assert!(absent.contains("scriv config init"), "{absent}");
    }

    #[test]
    fn an_owner_with_nothing_to_show_names_the_archive_filter() {
        assert!(empty_message("acme", false).contains("--archived"));
        assert!(!empty_message("acme", true).contains("--archived"));
    }
}
