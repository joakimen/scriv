//! `scriv repo` — list and pick the Git repositories found under the
//! configured search paths.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context, Result, bail};

use crate::config::RepoDisplay;
use crate::gh::{self, Repo};
use crate::path::{display_path, expand_home_dir, relative_label};
use crate::pick::{Choice, PickItem, Preview};
use crate::repo::FoundRepo;
use crate::{Ctx, pick, repo, term};

/// Discover repositories, category-tagged and sorted by path.
fn discover(ctx: &Ctx) -> Result<Vec<FoundRepo>> {
    if ctx.config.root.is_none() && ctx.config.extra.is_empty() {
        anyhow::bail!(
            "no `root` configured in {}; run `scriv config init` to create a starter config",
            ctx.config_path.display()
        );
    }
    ctx.log.info(&format!(
        "settings: ignore = {}",
        ctx.config.ignore.join(", ")
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
    for repo in &repos {
        let path = repo.path.to_string_lossy();
        println!("{}", display_path(&path, ctx.home_str(), absolute));
    }
    Ok(())
}

/// ANSI 256-colour indices used to tint categories, in assignment order.
/// Standard hues (cyan, green, yellow, magenta, blue, red) so they read well and
/// follow the terminal theme; cycles if there are more categories than colours.
const CATEGORY_COLORS: &[u8] = &[6, 2, 3, 5, 4, 1];

/// Colour for a repository whose owner is in no configured category.
const UNCATEGORIZED_COLOR: u8 = 8; // bright black

/// Map each configured category to a colour, in config order.
///
/// [`UNCATEGORIZED`](crate::config::UNCATEGORIZED) is deliberately not in the
/// map: an uncategorised repository is not a category of its own competing for
/// a hue, it is the absence of one, and greys out.
fn category_colors(ctx: &Ctx) -> std::collections::HashMap<&str, u8> {
    ctx.config
        .owners
        .keys()
        .enumerate()
        .map(|(i, c)| (c.as_str(), CATEGORY_COLORS[i % CATEGORY_COLORS.len()]))
        .collect()
}

/// The preview for a repository: its current branch and working-tree state,
/// then recent commits.
///
/// That is what distinguishes two similarly named checkouts — which branch is
/// out, whether it is dirty, and what was last done there. Both commands are
/// local and take tens of milliseconds, which is the bar for running anything
/// per highlighted row.
///
/// `--no-optional-locks` matters here: a plain `git status` refreshes and
/// rewrites the index, so merely scrolling past a repository would take its
/// index lock and contend with whatever the user is running in it.
fn preview(path: &str) -> Preview {
    let repo = pick::quote(path);
    Preview::Command(format!(
        "git --no-optional-locks -C {repo} -c color.status=always status --short --branch \
         | head -n 10; \
         git --no-optional-locks -C {repo} log --color=always --max-count=20 --date=relative \
         --format='%C(auto)%h%C(reset)  %C(blue)%an%C(reset)  %C(green)%ad%C(reset)  %s'"
    ))
}

/// `scriv repo pick` — fuzzy-select one repository and print its absolute path.
///
/// Each row is prefixed with its category, coloured per category so a `work`
/// checkout is distinguishable from a personal one at a glance — several owners
/// can share one category and therefore one colour. Paths render per
/// `picker.display`. The printed path is always absolute so a shell shim can
/// `cd` to it directly.
pub fn pick(ctx: &Ctx) -> Result<()> {
    let repos = discover(ctx)?;
    let colors = category_colors(ctx);

    // Character count, not bytes: `{:<width$}` pads by characters, so a byte
    // length would over-pad a category label containing non-ASCII.
    let width = repos
        .iter()
        .map(|r| r.category.chars().count())
        .max()
        .unwrap_or(0);

    let mode = ctx.config.picker.display;
    let items: Vec<PickItem> = repos
        .iter()
        .map(|repo| {
            let abs = repo.path.to_string_lossy().into_owned();
            let shown = match mode {
                RepoDisplay::Relative => relative_label(&repo.path, &repo.root),
                RepoDisplay::Tilde => display_path(&abs, ctx.home_str(), false),
                RepoDisplay::Full => abs.clone(),
            };
            let label = format!("{category:<width$}  {shown}", category = repo.category);
            PickItem::new(label, abs.clone())
                .color(
                    colors
                        .get(repo.category.as_str())
                        .copied()
                        .unwrap_or(UNCATEGORIZED_COLOR),
                )
                .preview(preview(&abs))
        })
        .collect();

    let choice = pick::pick_one(items, "Pick a repository", &ctx.config.picker)?;
    println!("{choice}");
    Ok(())
}

// --- clone ------------------------------------------------------------------

/// How many clones run at once.
///
/// Cloning is network-bound, so this is well above the core count; the ceiling
/// is what a remote will accept from one user before it starts refusing, not
/// what the machine can compute.
const CLONE_CONCURRENCY: usize = 8;

/// Colour for a row that is already on disk. Grey reads as "not actionable",
/// which is exactly what an already-cloned repository is.
const PRESENT_COLOR: u8 = 8;

/// The configured root, expanded — the one directory clones are written to.
fn clone_root(ctx: &Ctx) -> Result<PathBuf> {
    let root = ctx.config.root.as_deref().ok_or_else(|| {
        anyhow::anyhow!(
            "no `root` configured in {}; `repo clone` writes to <root>/<owner>/<repo>",
            ctx.config_path.display()
        )
    })?;
    Ok(expand_home_dir(root, ctx.home()))
}

/// Where `owner/repo` belongs on disk.
///
/// The inverse of the discovery walk: a clone lands exactly where `repo pick`
/// would look for it, so cloning something is the same as adding it to the
/// picker.
fn destination(root: &Path, owner: &str, name: &str) -> PathBuf {
    root.join(owner).join(name)
}

/// Owners worth offering, most useful first: those named in the config, then
/// any already on disk under the root, then the user's own login and orgs.
///
/// The three sources cover each other's blind spots. Config is intent, and
/// ranks first. The filesystem catches owners cloned from but never configured.
/// `gh` catches the rest — and is the only one that works on a machine with
/// nothing cloned yet, which is when `clone` matters most.
fn owner_candidates(ctx: &Ctx, root: &Path) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    let mut push = |owner: &str, out: &mut Vec<String>| {
        let key = owner.to_ascii_lowercase();
        if !owner.is_empty() && seen.insert(key) {
            out.push(owner.to_string());
        }
    };

    for owner in ctx.config.known_owners() {
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

    let colors = category_colors(ctx);
    let items: Vec<PickItem> = candidates
        .iter()
        .map(|owner| {
            let category = ctx.config.category_of(owner);
            let label = match category {
                Some(c) => format!("{owner}  ({c})"),
                None => owner.clone(),
            };
            PickItem::new(label, owner.clone()).color(
                category
                    .and_then(|c| colors.get(c).copied())
                    .unwrap_or(UNCATEGORIZED_COLOR),
            )
        })
        .collect();

    match pick::pick_one_or_query(items, "Owner (type any GitHub owner)", &ctx.config.picker)? {
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

/// Build the repository rows, marking the ones already on disk.
///
/// Present repositories stay in the list rather than being filtered out: their
/// absence would read as "this org does not have that repo", when the truth is
/// the opposite and useful — you already have it.
fn repo_items(repos: &[Repo], root: &Path) -> Vec<PickItem> {
    let width = repos
        .iter()
        .map(|r| r.name().chars().count())
        .max()
        .unwrap_or(0);
    // Rendered once and reused for both the column width and the rows; the
    // widest tag string cannot be known without formatting every one of them,
    // and formatting them twice is the easy mistake.
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
            let item = PickItem::new(label.trim_end(), repo.name_with_owner.clone())
                .preview(repo_preview(repo, present.then_some(dest.as_path())));
            if present {
                item.color(PRESENT_COLOR)
            } else {
                item
            }
        })
        .collect()
}

/// Clone `repos` into `root`, [`CLONE_CONCURRENCY`] at a time.
///
/// The workers pull from a shared queue rather than working through fixed
/// batches. Clone times vary by orders of magnitude — one large repository
/// among thirty small ones is the normal case — and a batch that waits for its
/// slowest member leaves every other worker idle until it finishes.
///
/// Every clone is reported, in the order it was requested rather than the order
/// it finished, so the summary is stable and readable. One failure does not
/// stop the others: a rate-limited or renamed repository should not cost you
/// the nine that were fine.
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
                    // A worker that panicked mid-clone poisons this; the
                    // results already collected are still worth reporting.
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
/// With no argument, pick an owner (from your config, your root, and `gh`, with
/// anything you type accepted), then fuzzy-select one or more of that owner's
/// repositories. `owner/repo` skips both pickers.
///
/// Everything lands at `<root>/<owner>/<repo>`, which is where discovery looks,
/// so a clone is in `repo pick` immediately afterwards.
pub fn clone(ctx: &Ctx, target: Option<&str>, limit: usize) -> Result<()> {
    let root = clone_root(ctx)?;

    // `owner/repo` names a repository outright; there is nothing to choose.
    if let Some(slug) = target.filter(|t| t.contains('/')) {
        return finish(ctx, &root, vec![slug.to_string()]);
    }

    let owner = match target {
        Some(owner) => owner.to_string(),
        None => select_owner(ctx, &root)?,
    };

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

    let chosen = pick::pick_many(
        repo_items(&repos, &root),
        "Repositories to clone (tab to select several)",
        &ctx.config.picker,
    )?;
    if chosen.is_empty() {
        return Ok(());
    }
    finish(ctx, &root, chosen)
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

    /// A clone must land exactly where discovery looks for it, or cloning
    /// something would not put it in the picker.
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

    /// Only what is unusual gets a tag; a public, live, non-fork repository is
    /// the default and would just add noise to every row.
    #[test]
    fn tags_only_mark_the_unusual() {
        let repos = repos();
        assert_eq!(repos[0].tags(), vec!["private"]);
        assert_eq!(repos[1].tags(), vec!["archived", "fork"]);
    }

    /// Repositories already on disk stay listed — their absence would read as
    /// "the org does not have that repo" — but are marked and greyed.
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

    /// The preview says where it already is — the reason the row is greyed.
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
