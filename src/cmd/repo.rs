//! `scriv repo` — list and pick the Git repositories found under the
//! configured search paths.

use anyhow::{Context, Result};

use crate::config::RepoDisplay;
use crate::path::{display_path, relative_label};
use crate::pick::{PickItem, Preview};
use crate::repo::FoundRepo;
use crate::{Ctx, pick, repo};

/// Discover repositories, group-tagged and sorted by path.
fn discover(ctx: &Ctx) -> Result<Vec<FoundRepo>> {
    if ctx.config.paths.is_empty() {
        anyhow::bail!(
            "no repository paths configured in {}; run `scriv config init` to create a starter config",
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

/// ANSI 256-colour indices used to tint groups, in assignment order. Standard
/// hues (cyan, green, yellow, magenta, blue, red) so they read well and follow
/// the terminal theme; cycles if there are more groups than colours.
const GROUP_COLORS: &[u8] = &[6, 2, 3, 5, 4, 1];

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
/// Each row is prefixed with its group name, coloured per group so groups are
/// easy to tell apart. Paths render per `picker.display`. The printed path is
/// always absolute so a shell shim can `cd` to it directly.
pub fn pick(ctx: &Ctx) -> Result<()> {
    let repos = discover(ctx)?;

    // Assign each configured group a colour, in config order.
    let colors: std::collections::HashMap<&str, u8> = ctx
        .config
        .paths
        .keys()
        .enumerate()
        .map(|(i, group)| (group.as_str(), GROUP_COLORS[i % GROUP_COLORS.len()]))
        .collect();
    // Character count, not bytes: `{:<width$}` pads by characters, so a byte
    // length would over-pad a group label containing non-ASCII.
    let width = repos
        .iter()
        .map(|r| r.group.chars().count())
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
            let label = format!("{group:<width$}  {shown}", group = repo.group);
            let mut item = PickItem::new(label, abs.clone()).preview(preview(&abs));
            if let Some(&color) = colors.get(repo.group.as_str()) {
                item = item.color(color);
            }
            item
        })
        .collect();

    let choice = pick::pick_one(items, "Pick a repository", &ctx.config.picker)?;
    println!("{choice}");
    Ok(())
}
