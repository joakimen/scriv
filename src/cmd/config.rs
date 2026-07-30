//! `scriv config` — inspect and generate the configuration file.

use std::os::unix::fs::DirBuilderExt;
use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::Ctx;

/// A commented starter config. Settings are grouped by the command that reads
/// them; users edit it to taste.
const TEMPLATE: &str = r#"# scriv configuration. Settings are grouped by the command that reads them,
# with `[picker]` — shared by every picker — at the end.

# `scriv repo`: where your repositories are, and how they are labelled.
[repo]

# Every repository lives under one root, laid out as <owner>/<repo> — the same
# shape as GitHub itself. `repo clone` writes here, so a clone always lands
# somewhere `repo pick` will find it.
root = "~/dev/github.com"

# Repositories outside the root, listed one at a time. An escape hatch for
# checkouts that predate the layout; `clone` never writes here.
# extra = ["~/bin"]

# Directory names to skip while searching.
ignore = ["node_modules", "target"]

# How repository paths are rendered: relative | tilde | full
# display = "relative"

# Labels name owners, one label to many owners, so everything you touch for work
# colours as one group in the picker however many orgs it spans. An owner with
# no label still shows up — just uncoloured.
#
# Written inline, on one line, so it stays an ordinary `[repo]` key: a
# `[repo.labels]` header would swallow every `[repo]` key written after it.
# labels = { personal = ["your-github-user"], work = ["acme", "acme-labs"] }

# The built-in fuzzy picker, shared by every command that opens one.
[picker]
height = "50%"        # finder height, e.g. "50%" or "20"
# preview = true       # show a preview pane for the highlighted row
# preview_window = "right:50%" # preview layout: [up|down|left|right][:SIZE][:hidden]
"#;

/// `scriv config init` — write `config.toml` into the config directory,
/// refusing to clobber an existing config (either format) unless `force`.
pub fn init(ctx: &Ctx, force: bool) -> Result<()> {
    let dir = ctx
        .config_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let target = dir.join("config.toml");
    let legacy = dir.join("config.json");

    if !force {
        if target.exists() {
            bail!(
                "configuration already exists at {} (pass --force to overwrite)",
                target.display()
            );
        }
        if legacy.exists() {
            bail!(
                "a legacy JSON configuration already exists at {}; \
                 pass --force to write config.toml alongside it",
                legacy.display()
            );
        }
    }

    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .with_context(|| format!("creating config directory {}", dir.display()))?;
    std::fs::write(&target, TEMPLATE).with_context(|| format!("writing {}", target.display()))?;

    println!("Wrote starter configuration to {}", target.display());
    println!("Edit it, then run `scriv config print` to verify.");
    Ok(())
}

/// `scriv config print` — print the resolved paths and ignore list.
pub fn print(ctx: &Ctx) -> Result<()> {
    ctx.log.info(&format!(
        "printing configuration from {}",
        ctx.config_path.display()
    ));

    let repo = &ctx.config.repo;
    println!("[repo]");
    println!("root: {}", repo.root.as_deref().unwrap_or("(unset)"));
    if !repo.extra.is_empty() {
        println!("extra: {}", repo.extra.join(", "));
    }
    println!("ignore: {}", repo.ignore.join(", "));
    println!("display: {}", repo.display.as_str());
    if !repo.labels.is_empty() {
        println!("labels:");
        for (label, owners) in &repo.labels {
            println!("  {label}: {}", owners.join(", "));
        }
    }

    println!();
    println!(
        "editor: {}",
        ctx.editor_setting()
            .unwrap_or("(unset — set $VISUAL or $EDITOR)")
    );
    Ok(())
}

/// `scriv config path` — print the resolved config file path.
pub fn path(ctx: &Ctx) -> Result<()> {
    println!("{}", ctx.config_path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{RepoDisplay, load_config};

    /// The starter config is the main thing teaching the current layout, so it
    /// has to be a config scriv actually accepts — a template still written in
    /// a superseded shape would hand every new user the migration error on
    /// their first run.
    #[test]
    fn the_starter_template_parses() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, TEMPLATE).unwrap();

        let cfg = load_config(&path).unwrap();
        assert_eq!(cfg.repo.root.as_deref(), Some("~/dev/github.com"));
        assert_eq!(
            cfg.repo.ignore,
            vec!["node_modules".to_string(), "target".to_string()]
        );
        assert_eq!(cfg.repo.display, RepoDisplay::Relative);
        assert_eq!(cfg.picker.height, "50%");
    }

    /// Every commented-out key is advice, and advice that does not parse is
    /// worse than none — uncommenting them all has to still yield a valid
    /// config, including the inline `labels` table.
    ///
    /// A commented key is `# <name> = ...`; prose comments are left alone, so
    /// this stays a test of the suggestions rather than of the heuristic.
    #[test]
    fn the_templates_commented_keys_parse_when_uncommented() {
        let is_key = |line: &str| {
            line.split_once(" = ").is_some_and(|(name, _)| {
                !name.is_empty() && name.chars().all(|c| c.is_ascii_lowercase() || c == '_')
            })
        };
        let uncommented: String = TEMPLATE
            .lines()
            .map(|line| match line.strip_prefix("# ") {
                Some(rest) if is_key(rest) => rest,
                _ => line,
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(uncommented.contains("labels = {"), "{uncommented}");

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, &uncommented).unwrap();

        let cfg = load_config(&path)
            .unwrap_or_else(|e| panic!("uncommented template rejected: {e:#}\n\n{uncommented}"));
        assert_eq!(cfg.repo.extra, vec!["~/bin".to_string()]);
        assert_eq!(cfg.repo.display, RepoDisplay::Relative);
        assert_eq!(cfg.repo.label_of("acme"), Some("work"));
        assert!(!cfg.picker.preview_window.is_empty());
    }
}
