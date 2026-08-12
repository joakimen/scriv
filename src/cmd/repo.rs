//! `scriv repo` — list and select the Git repositories found under the
//! configured search paths.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context, Result, bail};

use crate::config::{Labels, RepoDisplay};
use crate::gh::{self, Repo};
use crate::path::{display_path, expand_home_dir, relative_label};
use crate::repo::FoundRepo;
use crate::select::{Choice, Preview, SelectItem};
use crate::{Ctx, git, repo, select, term};

/// Discover repositories, label-tagged and sorted by path.
fn discover(ctx: &Ctx) -> Result<Vec<FoundRepo>> {
    if ctx.config.repo.root.is_none() && ctx.config.repo.extra.is_empty() {
        anyhow::bail!(
            "no `root` configured in {}; run `scriv config init` to create a starter config",
            ctx.config_path.display()
        );
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
    Ok(())
}

/// ANSI 256-colour indices used to tint labels, in assignment order.
/// Standard hues (cyan, green, yellow, magenta, blue, red) so they read well and
/// follow the terminal theme; cycles if there are more labels than colours.
const LABEL_COLORS: &[u8] = &[6, 2, 3, 5, 4, 1];

/// Labels whose colour is fixed by name rather than by config order, so the hue
/// means the same thing in every checkout. Everything else takes the next
/// unused hue, in config order.
const NAMED_LABEL_COLORS: &[(&str, u8)] = &[("work", 6), ("personal", 2)];

/// The fixed colour for `label`, if it is one of the conventional names.
fn named_color(label: &str) -> Option<u8> {
    NAMED_LABEL_COLORS
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(label))
        .map(|&(_, color)| color)
}

/// Map each configured label to a colour.
/// [`UNLABELLED`](crate::config::UNLABELLED) is deliberately absent: it is not a
/// label competing for a hue, and stays the terminal's default foreground.
fn label_colors(labels: &Labels) -> std::collections::HashMap<&str, u8> {
    // Hues spoken for by a named label actually present, so an unnamed label
    // never collides with `work`'s cyan.
    let taken: Vec<u8> = labels.keys().filter_map(|l| named_color(l)).collect();
    let mut free: Vec<u8> = LABEL_COLORS
        .iter()
        .copied()
        .filter(|c| !taken.contains(c))
        .collect();
    if free.is_empty() {
        free = LABEL_COLORS.to_vec();
    }

    let mut next = 0;
    labels
        .keys()
        .map(|label| {
            let color = named_color(label).unwrap_or_else(|| {
                let color = free[next % free.len()];
                next += 1;
                color
            });
            (label.as_str(), color)
        })
        .collect()
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
            let row = format!("{label:<width$}  {shown}", label = repo.label);
            let item = SelectItem::new(row, abs.clone()).preview(select::checkout_preview(&abs));
            match colors.get(repo.label.as_str()) {
                Some(&color) => item.color(color),
                None => item,
            }
        })
        .collect()
}

/// `scriv repo sel` — fuzzy-select one repository and print its absolute path.
///
/// The path is printed absolute so a shell shim can `cd` to it directly.
pub fn sel(ctx: &Ctx) -> Result<()> {
    let repos = discover(ctx)?;
    let rows = repo_rows(ctx, &repos);
    let choice = select::select_one(rows, "Select a repository", &ctx.config.selector)?;
    println!("{choice}");
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
    let rows = repo_rows(ctx, &repos);
    let choice = select::select_one(rows, "Open a repository on GitHub", &ctx.config.selector)?;
    gh::view_repo_web(Path::new(&choice))
}

// --- clone ------------------------------------------------------------------

/// How many clones run at once. Network-bound, so well above the core count;
/// the ceiling is what a remote accepts from one user.
const CLONE_CONCURRENCY: usize = 8;

/// Colour for a row that is already on disk. Grey reads as "not actionable",
/// which is exactly what an already-cloned repository is.
const PRESENT_COLOR: u8 = 8;

/// The configured root, expanded — the one directory clones are written to.
fn clone_root(ctx: &Ctx) -> Result<PathBuf> {
    let root = ctx.config.repo.root.as_deref().ok_or_else(|| {
        anyhow::anyhow!(
            "no `root` configured in {}; `repo clone` writes to <root>/<owner>/<repo>",
            ctx.config_path.display()
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
    match gh::owners() {
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

/// The preview for a repository: description and the facts that tell two
/// similar repositories apart, from data `gh repo list` already returned.
fn repo_preview(repo: &Repo, present: Option<&Path>) -> Preview {
    let mut out = term::paint(&repo.name_with_owner, 6, true);
    out.push('\n');

    let mut facts = Vec::new();
    if !repo.language().is_empty() {
        facts.push(repo.language().to_string());
    }
    if !repo.pushed_date().is_empty() {
        facts.push(format!("pushed {}", repo.pushed_date()));
    }
    facts.extend(repo.tags().iter().map(|t| t.to_string()));
    if !facts.is_empty() {
        out.push_str(&facts.join("  ·  "));
        out.push('\n');
    }

    if let Some(path) = present {
        out.push_str(&term::paint(
            &format!("already cloned at {}\n", path.display()),
            PRESENT_COLOR,
            true,
        ));
    }
    out.push('\n');

    let description = repo.description.trim();
    out.push_str(if description.is_empty() {
        "(no description)"
    } else {
        description
    });
    Preview::Text(out)
}

/// Build the repository rows, marking the ones already on disk. They stay
/// listed rather than filtered out, since their absence would read as "this org
/// does not have that repo".
fn repo_items(repos: &[Repo], root: &Path) -> Vec<SelectItem> {
    let width = repos
        .iter()
        .map(|r| r.name().chars().count())
        .max()
        .unwrap_or(0);
    // Rendered once and reused for both the column width and the rows.
    let tags: Vec<String> = repos.iter().map(|r| r.tags().join(" ")).collect();
    let tag_width = tags.iter().map(|t| t.chars().count()).max().unwrap_or(0);

    repos
        .iter()
        .zip(&tags)
        .map(|(repo, tags)| {
            let dest = destination(root, repo.owner(), repo.name());
            let present = dest.exists();
            let marker = if present { "✓" } else { " " };
            let label = format!(
                "{marker} {name:<width$}  {tags:<tag_width$}  {description}",
                name = repo.name(),
                description = repo.description.trim(),
            );
            let item = SelectItem::new(label.trim_end(), repo.name_with_owner.clone())
                .preview(repo_preview(repo, present.then_some(dest.as_path())));
            if present {
                item.color(PRESENT_COLOR)
            } else {
                item
            }
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
/// `owner/repo` skips both selectors. Everything lands at
/// `<root>/<owner>/<repo>`, which is where discovery looks.
pub fn clone(ctx: &Ctx, target: Option<&str>, limit: usize) -> Result<()> {
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

    let repos = gh::list_repos(&owner, limit)?;
    ctx.log
        .info(&format!("{} repositories for {owner}", repos.len()));
    if repos.is_empty() {
        bail!("no repositories found for {owner}");
    }
    if repos.len() == limit {
        eprintln!(
            "note: showing the first {limit} repositories for {owner}; pass --limit to raise it"
        );
    }

    let chosen = select::select_many(
        repo_items(&repos, &root),
        "Repositories to clone (tab to select several)",
        &ctx.config.selector,
    )?;
    if chosen.is_empty() {
        return Ok(());
    }
    finish(ctx, &root, chosen)
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

    fn repos() -> Vec<Repo> {
        gh::parse_repos(
            r#"[
            {"nameWithOwner":"acme/billing-api","description":"Meters usage",
             "isPrivate":true,"isArchived":false,"isFork":false,
             "primaryLanguage":{"name":"Rust"},"pushedAt":"2026-07-27T09:12:33Z"},
            {"nameWithOwner":"acme/old-thing","description":"",
             "isPrivate":false,"isArchived":true,"isFork":true,
             "primaryLanguage":null,"pushedAt":"2024-01-02T00:00:00Z"}
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

    #[test]
    fn present_repositories_are_marked_not_hidden() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("acme/billing-api")).unwrap();

        let items = repo_items(&repos(), root);
        assert_eq!(items.len(), 2, "a present repo was dropped from the list");
        assert!(items[0].label.starts_with('✓'), "{}", items[0].label);
        assert_eq!(items[0].color, Some(PRESENT_COLOR));
        assert!(!items[1].label.starts_with('✓'), "{}", items[1].label);
        assert_eq!(items[1].color, None);
        // The value is still the slug, so selecting it is well-defined.
        assert_eq!(items[0].value(), "acme/billing-api");
    }

    #[test]
    fn preview_names_the_existing_checkout() {
        let path = PathBuf::from("/home/u/dev/github.com/acme/billing-api");
        let Preview::Text(text) = repo_preview(&repos()[0], Some(&path)) else {
            panic!("a repo preview must not spawn a command");
        };
        assert!(text.contains("already cloned at"), "{text}");
        assert!(text.contains("Rust"), "{text}");
        assert!(text.contains("Meters usage"), "{text}");
    }

    #[test]
    fn preview_handles_a_missing_description() {
        let Preview::Text(text) = repo_preview(&repos()[1], None) else {
            panic!("expected text");
        };
        assert!(text.contains("(no description)"), "{text}");
        assert!(!text.contains("already cloned"), "{text}");
    }
}
