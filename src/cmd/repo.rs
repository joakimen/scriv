//! `scriv repo` — list and pick discovered Git repositories.

use anyhow::{Context, Result};

use crate::path::display_path;
use crate::{Ctx, pick, repo};

/// Discover repositories, sorted, home-collapsed unless `absolute`.
fn discover(ctx: &Ctx) -> Result<Vec<String>> {
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
    repos.sort_by(|a, b| a.as_os_str().cmp(b.as_os_str()));

    if repos.is_empty() {
        anyhow::bail!("no repositories found");
    }
    Ok(repos
        .iter()
        .map(|r| r.to_string_lossy().into_owned())
        .collect())
}

/// `scriv repo ls` — print every discovered repository, one per line.
pub fn ls(ctx: &Ctx, absolute: bool) -> Result<()> {
    let repos = discover(ctx)?;
    ctx.log
        .info(&format!("returning {} repositories", repos.len()));
    for repo in &repos {
        println!("{}", display_path(repo, ctx.home_str(), absolute));
    }
    Ok(())
}

/// `scriv repo pick` — fuzzy-select one repository and print its absolute path.
///
/// The printed path is always absolute so a shell shim can `cd` to it without
/// re-expanding `~`.
pub fn pick(ctx: &Ctx) -> Result<()> {
    let repos = discover(ctx)?;
    let choice = pick::pick_one(&repos, "Pick a repository", &ctx.config.picker)?;
    println!("{choice}");
    Ok(())
}
