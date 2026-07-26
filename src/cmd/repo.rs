//! `scriv repo` — list and pick discovered Git repositories.

use anyhow::{Context, Result};

use crate::path::display_path;
use crate::pick::PickItem;
use crate::repo::FoundRepo;
use crate::{Ctx, pick, repo};

/// Discover repositories, group-tagged and sorted by path.
fn discover(ctx: &Ctx) -> Result<Vec<FoundRepo>> {
    if ctx.config.paths.is_empty() {
        anyhow::bail!(
            "no repository paths configured in {}",
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
        anyhow::bail!("no repositories found");
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

/// `scriv repo pick` — fuzzy-select one repository and print its absolute path.
///
/// The picker shows `~`-collapsed paths, and — when more than one group is
/// configured — an aligned group tag so the repo's group is clear. The printed
/// path is always absolute so a shell shim can `cd` to it directly.
pub fn pick(ctx: &Ctx) -> Result<()> {
    let repos = discover(ctx)?;

    let show_groups = ctx.config.paths.len() > 1;
    let width = if show_groups {
        repos.iter().map(|r| r.group.len()).max().unwrap_or(0)
    } else {
        0
    };

    let items: Vec<PickItem> = repos
        .iter()
        .map(|repo| {
            let abs = repo.path.to_string_lossy().into_owned();
            let shown = display_path(&abs, ctx.home_str(), false);
            let label = if show_groups {
                format!("{group:<width$}  {shown}", group = repo.group)
            } else {
                shown
            };
            PickItem::new(label, abs)
        })
        .collect();

    let choice = pick::pick_one(items, "Pick a repository", &ctx.config.picker)?;
    println!("{choice}");
    Ok(())
}
