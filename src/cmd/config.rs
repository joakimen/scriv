//! `scriv config` — inspect, generate, and check the configuration file.

use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

use crate::path::expand_home_dir;
use crate::{Ctx, cmd, files, gh, history, repo, term};

/// A commented starter config. Settings are grouped by the command that reads
/// them; users edit it to taste.
const TEMPLATE: &str = r#"# scriv configuration. Settings are grouped by the command that reads them,
# with `[selector]` — shared by every selector — at the end.

# `scriv repo`: where your repositories are, and how they are labelled.
[repo]

# Every repository lives under one root, laid out as <owner>/<repo> — the same
# shape as GitHub itself. `repo clone` writes here, so a clone always lands
# somewhere `repo sel` will find it.
root = "~/dev/github.com"

# Repositories outside the root, listed one at a time. An escape hatch for
# checkouts that predate the layout; `clone` never writes here.
# extra = ["~/bin"]

# Directory names to skip while searching.
ignore = ["node_modules", "target"]

# How repository paths are rendered: relative | tilde | full
# display = "relative"

# Labels name owners, one label to many owners, so everything you touch for work
# colours as one group in the selector however many orgs it spans. An owner with
# no label still shows up — just uncoloured.
#
# Written inline, on one line, so it stays an ordinary `[repo]` key: a
# `[repo.labels]` header would swallow every `[repo]` key written after it.
# labels = { personal = ["your-github-user"], work = ["acme", "acme-labs"] }

# `scriv worktree`: where `worktree add` creates a tree.
[worktree]

# One directory per branch, with `/` written as `-`. A relative path is inside
# the repository the tree belongs to, and is added to that clone's
# .git/info/exclude the first time, so nothing else offers the tree twice. An
# absolute path holds the trees of every repository, under the repository's own
# name.
# root = ".worktrees"

# `scriv note`: where your notes are, and what opens one.
[note]

# The directory holding them — an Obsidian vault, or any tree of Markdown
# files. Notes below it are listed most recently modified first, and what each
# one is called, which tags it carries and when it was created come from its
# YAML front matter. Without this, `scriv note` has nowhere to look.
# root = "~/notes"

# Labels for the directories directly below the root, one label to many
# directories — the same idea as `[repo] labels`, and the same colours, so
# `work` reads the same in both. A directory with no label still shows up in
# the label column, under its own name and uncoloured.
#
# Written inline, on one line, for the reason `[repo] labels` is.
# labels = { work = ["projects", "clients"], personal = ["journal"] }

# The one permanent note `note scratch` opens — somewhere to put a thought
# without deciding first whether it is worth a note of its own. A path below
# the root; its directory is created the first time.
# scratch = "scratch/scratch.md"

# What `note edit` launches, split on whitespace like $EDITOR. Its own setting
# because a note is as often read as written — `glow` and `nvim` are both
# answers. Unset, it is $VISUAL then $EDITOR, as `scriv edit` uses.
# editor = "nvim"

# `scriv history`: which shell history to search.
[history]

# fish's history file. The default is $XDG_DATA_HOME/fish/fish_history, falling
# back to ~/.local/share/fish/fish_history. Set this only if you have named your
# session — `set -U fish_history work` reads `work_history` instead — since fish
# does not export that variable for scriv to find.
# file = "~/.local/share/fish/work_history"

# The built-in fuzzy selector, shared by every command that opens one.
[selector]
height = "50%"        # finder height, e.g. "50%" or "20"
# recent = true        # offer repositories and files you have chosen before first
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
    println!("[note]");
    println!(
        "root: {}",
        ctx.config.note.root.as_deref().unwrap_or("(unset)")
    );
    if !ctx.config.note.labels.is_empty() {
        println!("labels:");
        for (label, dirs) in &ctx.config.note.labels {
            println!("  {label}: {}", dirs.join(", "));
        }
    }
    println!(
        "scratch: {}",
        ctx.config
            .note
            .scratch
            .as_deref()
            .unwrap_or(crate::note::DEFAULT_SCRATCH)
    );
    println!(
        "editor: {}",
        ctx.note_editor_setting()
            .unwrap_or("(unset — set `[note] editor`, $VISUAL or $EDITOR)")
    );

    println!();
    println!("[history]");
    println!("file: {}", ctx.history_path.display());

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

// --- check ------------------------------------------------------------------

/// How one check came out. Only [`Status::Fail`] means something is actually
/// broken, which is what makes the command's exit status worth anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Ok,
    Warn,
    Fail,
}

impl Status {
    /// A shape, not a colour alone, so a piped report says exactly as much.
    fn glyph(self) -> &'static str {
        match self {
            Self::Ok => "✓",
            Self::Warn => "!",
            Self::Fail => "✗",
        }
    }

    /// ANSI 256-colour index: green, yellow, red.
    fn color(self) -> u8 {
        match self {
            Self::Ok => 2,
            Self::Warn => 3,
            Self::Fail => 1,
        }
    }
}

/// One thing that was looked at, and what was found.
#[derive(Debug, Clone)]
struct Check {
    /// What was looked at, e.g. `repo root`.
    name: &'static str,
    status: Status,
    /// What was found, and — when something is wrong — what to do about it.
    detail: String,
}

impl Check {
    fn new(name: &'static str, status: Status, detail: impl Into<String>) -> Self {
        Self {
            name,
            status,
            detail: detail.into(),
        }
    }

    fn ok(name: &'static str, detail: impl Into<String>) -> Self {
        Self::new(name, Status::Ok, detail)
    }

    fn warn(name: &'static str, detail: impl Into<String>) -> Self {
        Self::new(name, Status::Warn, detail)
    }

    fn fail(name: &'static str, detail: impl Into<String>) -> Self {
        Self::new(name, Status::Fail, detail)
    }
}

/// Render one check as its row: glyph, aligned name, detail. `width` is the
/// widest name in the report, counted in characters.
fn render(check: &Check, width: usize, color: bool) -> String {
    let row = format!(
        "{glyph} {name:<width$}  {detail}",
        glyph = check.status.glyph(),
        name = check.name,
        detail = check.detail,
    );
    term::paint(row.trim_end(), check.status.color(), color)
}

/// How many checks failed. Warnings are not failures: the command exits
/// non-zero only when something is genuinely broken, so it is worth putting in
/// a setup script.
fn failures(checks: &[Check]) -> usize {
    checks.iter().filter(|c| c.status == Status::Fail).count()
}

/// Resolve `program` against `path_env` the way a shell would: a name
/// containing a separator is a path already, anything else is looked for in
/// each `PATH` entry in order. Executability is deliberately not checked — the
/// spawn reports that better.
fn resolve_on_path(
    program: &str,
    path_env: Option<&str>,
    exists: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    if program.contains(std::path::MAIN_SEPARATOR) {
        let direct = PathBuf::from(program);
        return exists(&direct).then_some(direct);
    }
    path_env?
        .split(':')
        .filter(|dir| !dir.is_empty())
        .map(|dir| Path::new(dir).join(program))
        .find(|candidate| exists(candidate))
}

/// [`resolve_on_path`] against this process's `PATH` and the filesystem.
fn on_path(program: &str) -> Option<PathBuf> {
    resolve_on_path(program, std::env::var("PATH").ok().as_deref(), |p| {
        p.exists()
    })
}

/// The first line of `program --version`, for reporting which one is installed.
fn version_of(program: &str) -> Option<String> {
    let output = Command::new(program)
        .arg("--version")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines().next().map(str::trim).map(str::to_string)
}

/// Whether a tool scriv shells out to is installed, and which version.
fn tool_check(name: &'static str, program: &str, required: bool, note: &str) -> Check {
    match on_path(program) {
        Some(_) => Check::ok(name, version_of(program).unwrap_or_else(|| "found".into())),
        None if required => Check::fail(name, format!("not on PATH — {note}")),
        None => Check::warn(name, format!("not on PATH — {note}")),
    }
}

/// What the `gh` row says when `gh` is not installed at all.
const GH_NOTE: &str = "only `pr` and `repo clone`/`open` need it (https://cli.github.com)";

/// What the `gh` row says once it is installed: which version, and whether it
/// can act as anyone. An expired token fails `pr` exactly as completely as a
/// missing binary does, which is why one row covers both — but neither stops
/// the rest of scriv, so it is a warning.
fn gh_state(version: &str, authenticated: bool) -> (Status, String) {
    if authenticated {
        (Status::Ok, format!("{version}, authenticated"))
    } else {
        (
            Status::Warn,
            format!("{version}, not authenticated — run `gh auth login`"),
        )
    }
}

/// `gh`: installed, and logged in. The login is asked of GitHub, which is the
/// slowest thing in the report and the reason this is not a [`tool_check`].
fn gh_check() -> Check {
    if on_path("gh").is_none() {
        return Check::warn("gh", format!("not on PATH — {GH_NOTE}"));
    }
    let version = version_of("gh").unwrap_or_else(|| "found".into());
    let (status, detail) = gh_state(&version, gh::authenticated());
    Check::new("gh", status, detail)
}

/// The config file itself: whether there is one, and where. Reaching this point
/// means it parsed, so the only question left is whether one exists — and
/// absent is a warning, since the known-files commands work without one.
fn config_check(ctx: &Ctx) -> Check {
    let path = ctx.config_path.display().to_string();
    if ctx.config_path.exists() {
        Check::ok("config", path)
    } else {
        Check::warn(
            "config",
            format!("{path} not found — run `scriv config init`"),
        )
    }
}

/// The search root, and the `extra` paths beside it.
fn root_checks(ctx: &Ctx) -> Vec<Check> {
    let mut checks = Vec::new();

    match &ctx.config.repo.root {
        None if ctx.config.repo.extra.is_empty() => {
            checks.push(Check::fail(
                "repo root",
                "not set — `repo`, `edit --tracked` and `repo clone` have nowhere to look",
            ));
        }
        None => checks.push(Check::warn(
            "repo root",
            "not set; only `extra` is searched",
        )),
        Some(root) => {
            let expanded = expand_home_dir(root, ctx.home());
            let shown = expanded.display();
            checks.push(if expanded.is_dir() {
                Check::ok("repo root", format!("{shown}"))
            } else {
                Check::fail("repo root", format!("{shown} is not a directory"))
            });
        }
    }

    if !ctx.config.repo.extra.is_empty() {
        let total = ctx.config.repo.extra.len();
        let missing: Vec<&str> = ctx
            .config
            .repo
            .extra
            .iter()
            .filter(|p| !expand_home_dir(p, ctx.home()).is_dir())
            .map(String::as_str)
            .collect();
        checks.push(if missing.is_empty() {
            Check::ok("repo extra", format!("{total} path(s), all present"))
        } else {
            // Discovery treats a missing search path as a hard error.
            Check::fail("repo extra", format!("missing: {}", missing.join(", ")))
        });
    }

    checks
}

/// How many repositories discovery actually finds. Skipped when a search path
/// has already been reported missing, so the report stays one line per thing
/// wrong.
fn discovery_check(ctx: &Ctx, paths_ok: bool) -> Option<Check> {
    if !paths_ok || (ctx.config.repo.root.is_none() && ctx.config.repo.extra.is_empty()) {
        return None;
    }
    Some(
        match repo::find_all_repos(&ctx.config, ctx.home(), &ctx.log) {
            Ok(repos) if repos.is_empty() => Check::warn(
                "repositories",
                "none found — check `root` and `ignore` in the config",
            ),
            Ok(repos) => Check::ok("repositories", format!("{} found", repos.len())),
            Err(err) => Check::fail("repositories", format!("{err:#}")),
        },
    )
}

/// The editor `scriv edit` will launch, and whether it is actually there.
fn editor_check(ctx: &Ctx) -> Check {
    let Some(setting) = ctx.editor_setting() else {
        return Check::warn(
            "editor",
            "no $VISUAL or $EDITOR — `scriv edit` has nothing to open files with",
        );
    };
    // The setting may carry arguments (`code -w`); only the program is
    // looked for, the same split the launch does.
    let program = setting.split_whitespace().next().unwrap_or(setting);
    match on_path(program) {
        Some(found) => Check::ok("editor", format!("{setting} ({})", found.display())),
        None => Check::fail("editor", format!("{setting} — `{program}` is not on PATH")),
    }
}

/// The notes vault: where it is, and how many notes are in it.
///
/// One row rather than two — a root that resolves and a count of what is under
/// it are the same question — and a warning rather than a failure when it is
/// unset, since every other group works without one.
fn note_checks(ctx: &Ctx) -> Vec<Check> {
    let mut checks = Vec::new();

    if ctx.config.note.root.is_none() {
        checks.push(Check::warn(
            "note vault",
            "`[note] root` not set — `scriv note` has nowhere to look",
        ));
    } else {
        checks.push(match cmd::note::vault_summary(ctx) {
            Err(err) => Check::fail("note vault", format!("{err:#}")),
            Ok((root, 0)) => Check::warn(
                "note vault",
                format!("{} holds no Markdown files", root.display()),
            ),
            Ok((root, count)) => Check::ok(
                "note vault",
                format!("{count} note(s) in {}", root.display()),
            ),
        });
    }

    // Only when it is set: unset, it is the editor the row above already
    // reported, and a second row saying so is a second row saying so.
    if let Some(setting) = ctx.config.note.editor.as_deref() {
        let program = setting.split_whitespace().next().unwrap_or(setting);
        checks.push(match on_path(program) {
            Some(found) => Check::ok("note editor", format!("{setting} ({})", found.display())),
            None => Check::fail(
                "note editor",
                format!("{setting} — `{program}` is not on PATH"),
            ),
        });
    }

    checks
}

/// fish's history file: whether it is where scriv looked, and how much is in
/// it.
fn history_check(ctx: &Ctx) -> Check {
    let shown = ctx.history_path.display().to_string();
    match std::fs::read(&ctx.history_path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Check::warn(
            "fish history",
            format!("{shown} not found — set `[history] file` if yours is elsewhere"),
        ),
        Err(e) => Check::fail("fish history", format!("{shown}: {e}")),
        Ok(data) => {
            // The same list `history sel` offers, so the count is the one the
            // selector will show rather than one that counts key presses.
            let entries = history::recent_first(history::typed_only(history::parse(
                &String::from_utf8_lossy(&data),
            )));
            Check::ok(
                "fish history",
                format!("{} unique command(s) in {shown}", entries.len()),
            )
        }
    }
}

/// The known-files list, and how much of it still exists on disk.
fn files_check(ctx: &Ctx) -> Check {
    let lines = match files::read_lines(&ctx.files_path) {
        Ok(lines) => lines,
        Err(err) => return Check::fail("tracked files", format!("{err:#}")),
    };
    if lines.is_empty() {
        return Check::ok("tracked files", "none yet — add one with `scriv file add`");
    }
    let missing = lines
        .iter()
        .filter(|line| !Path::new(&crate::path::expand_tilde(line, ctx.home_str())).exists())
        .count();
    if missing == 0 {
        Check::ok("tracked files", format!("{} tracked", lines.len()))
    } else {
        // Not a failure: `file ls --missing` is the thing to run next.
        Check::warn(
            "tracked files",
            format!(
                "{} tracked, {missing} no longer on disk — see `scriv file ls --missing`",
                lines.len()
            ),
        )
    }
}

/// Everything `config check` looks at, in the order it is reported: the config
/// first, then what it points at, then the tools scriv shells out to.
fn collect(ctx: &Ctx) -> Vec<Check> {
    let mut checks = vec![config_check(ctx)];

    let paths = root_checks(ctx);
    let paths_ok = failures(&paths) == 0;
    checks.extend(paths);
    checks.extend(discovery_check(ctx, paths_ok));

    checks.push(editor_check(ctx));
    checks.push(tool_check(
        "git",
        "git",
        true,
        "`branch` and `repo` cannot work without it",
    ));
    checks.push(gh_check());
    checks.push(tool_check(
        "rg",
        "rg",
        false,
        "only `note rg` needs it (https://github.com/BurntSushi/ripgrep)",
    ));
    // `kill` and `lsof` get no row: both ship in the same base system `ps`
    // does, and a report of three lines saying the same thing is one line.
    checks.push(tool_check(
        "ps",
        "ps",
        true,
        "`proc` reads the process table through it",
    ));
    checks.push(history_check(ctx));
    checks.push(files_check(ctx));
    checks.extend(note_checks(ctx));
    checks
}

/// `scriv config check` — look at everything scriv depends on in one go and say
/// what is wrong with it. The exit status is non-zero only when something is
/// genuinely broken, so it is worth putting in a setup script.
pub fn check(ctx: &Ctx) -> Result<()> {
    let checks = collect(ctx);
    let color = ctx.color();
    let width = checks
        .iter()
        .map(|c| c.name.chars().count())
        .max()
        .unwrap_or(0);

    let mut out = term::Listing::stdout();
    for check in &checks {
        if !out.line(&render(check, width, color))? {
            return Ok(());
        }
    }
    out.finish()?;

    let failed = failures(&checks);
    if failed > 0 {
        bail!("{failed} of {} checks failed", checks.len());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{RepoDisplay, load_config};

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
        assert_eq!(cfg.selector.height, "50%");
    }

    /// A commented key is `# <name> = ...`; prose comments are left alone.
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
        assert!(!cfg.selector.preview_window.is_empty());
    }

    // --- check ---

    #[test]
    fn warnings_do_not_fail_the_run() {
        let checks = vec![
            Check::ok("git", "git version 2.51.0"),
            Check::warn("gh", "not on PATH"),
        ];
        assert_eq!(failures(&checks), 0);

        let broken = vec![Check::fail("repo root", "not set"), checks[1].clone()];
        assert_eq!(failures(&broken), 1);
    }

    #[test]
    fn every_status_is_a_distinct_shape_not_a_colour() {
        let glyphs: Vec<&str> = [Status::Ok, Status::Warn, Status::Fail]
            .iter()
            .map(|s| s.glyph())
            .collect();
        let mut unique = glyphs.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), glyphs.len(), "two statuses look alike");

        let plain = render(&Check::fail("repo root", "not set"), 9, false);
        assert_eq!(plain, "✗ repo root  not set");
        assert!(!plain.contains('\x1b'), "colour leaked into a plain report");
    }

    #[test]
    fn names_are_padded_into_one_column() {
        let rows = [
            render(&Check::ok("gh", "found"), 9, false),
            render(&Check::ok("repo root", "/tmp"), 9, false),
        ];
        let column = |row: &str| row.rfind("  ").map(|i| i + 2);
        assert_eq!(column(&rows[0]), column(&rows[1]), "{rows:?}");
    }

    #[test]
    fn an_unauthenticated_gh_is_a_warning_that_names_the_way_out() {
        let (status, detail) = gh_state("gh version 2.97.0", false);
        assert_eq!(status, Status::Warn, "a login nobody has is not fatal");
        assert!(detail.contains("gh auth login"), "{detail}");
        assert!(
            detail.contains("2.97.0"),
            "the version went missing: {detail}"
        );

        let (status, detail) = gh_state("gh version 2.97.0", true);
        assert_eq!(status, Status::Ok);
        assert!(!detail.contains("gh auth login"), "{detail}");
    }

    #[test]
    fn path_lookup_takes_the_first_match_in_order() {
        let found = resolve_on_path("gh", Some("/a:/b:/c"), |p| {
            p == Path::new("/b/gh") || p == Path::new("/c/gh")
        });
        assert_eq!(found, Some(PathBuf::from("/b/gh")));
    }

    #[test]
    fn path_lookup_reports_nothing_when_it_is_not_there() {
        assert_eq!(resolve_on_path("gh", Some("/a:/b"), |_| false), None);
        // No PATH at all is not a panic; it is simply nothing found.
        assert_eq!(resolve_on_path("gh", None, |_| true), None);
        // Empty entries are skipped rather than turned into a bare filename.
        assert_eq!(
            resolve_on_path("gh", Some("::"), |p| p == Path::new("gh")),
            None
        );
    }

    #[test]
    fn a_program_with_a_separator_is_used_as_it_stands() {
        assert_eq!(
            resolve_on_path("/opt/bin/hx", Some("/usr/bin"), |p| p
                == Path::new("/opt/bin/hx")),
            Some(PathBuf::from("/opt/bin/hx"))
        );
        assert_eq!(
            resolve_on_path("/opt/bin/hx", Some("/usr/bin"), |_| false),
            None
        );
    }
}
