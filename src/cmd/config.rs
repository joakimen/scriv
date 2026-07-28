//! `scriv config` — inspect and generate the configuration file.

use std::os::unix::fs::DirBuilderExt;
use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::Ctx;

/// A commented starter config covering the common layout: search roots grouped
/// by label and the default picker. Users edit it to taste.
const TEMPLATE: &str = r#"# scriv configuration.

# Every repository lives under one root, laid out as <owner>/<repo> — the same
# shape as GitHub itself. `repo clone` writes here, so a clone always lands
# somewhere `repo pick` will find it.
root = "~/dev/github.com"

# Repositories outside the root, listed one at a time. An escape hatch for
# checkouts that predate the layout; `clone` never writes here.
# extra = ["~/bin"]

# Directory names to skip while searching.
ignore = ["node_modules", "target"]

# Editor launched by `scriv edit`. Defaults to $VISUAL, then $EDITOR.
# editor = "nvim"

# Categories label owners, one category to many owners, so everything you touch
# for work colours as one group in the picker however many orgs it spans. An
# owner in no category still shows up — just uncoloured.
[owners]
# personal = ["your-github-user"]
# work = ["acme", "acme-labs"]

# Built-in fuzzy picker.
[picker]
height = "50%"        # finder height, e.g. "50%" or "20"
# display = "relative" # repo path rendering: relative | tilde | full
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

    println!("root: {}", ctx.config.root.as_deref().unwrap_or("(unset)"));
    if !ctx.config.extra.is_empty() {
        println!("extra: {}", ctx.config.extra.join(", "));
    }
    if !ctx.config.owners.is_empty() {
        println!("owners:");
        for (category, owners) in &ctx.config.owners {
            println!("  {category}: {}", owners.join(", "));
        }
    }
    println!();
    println!("ignore: {}", ctx.config.ignore.join(", "));
    println!(
        "editor: {}",
        ctx.editor_setting()
            .unwrap_or("(unset — set $EDITOR or $VISUAL)")
    );
    Ok(())
}

/// `scriv config path` — print the resolved config file path.
pub fn path(ctx: &Ctx) -> Result<()> {
    println!("{}", ctx.config_path.display());
    Ok(())
}
