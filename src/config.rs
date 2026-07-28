//! Configuration model and loading. Parsing and path resolution are pure —
//! they take environment and home values as arguments — so the only I/O is the
//! file read in [`load_config`].
//!
//! Two files live side by side under the config directory:
//!
//! - `config.toml` — hand-edited settings (repository paths, ignore list,
//!   picker preferences). A legacy `config.json` is still read when no TOML
//!   file is present.
//! - `files` — the known-files list, rewritten programmatically by
//!   `scriv file add`/`forget`/`prune`. Kept separate so machine writes never
//!   clobber hand-written settings or comments.

use anyhow::{Context, Result};
use indexmap::IndexMap;
use serde::Deserialize;
use std::path::{Path, PathBuf};

use crate::path::expand_home_dir;

/// Overrides the resolved config file path when set.
pub const CONFIG_ENV_VAR: &str = "SCRIV_CONFIG";
/// Base directory for the default config location, per the XDG spec.
pub const XDG_ENV_VAR: &str = "XDG_CONFIG_HOME";

const DEFAULT_IGNORED_DIRS: &[&str] = &["node_modules", "vendor", "dist", "build", "target"];

/// How deep below [`Config::root`] a repository sits: `<root>/<owner>/<repo>`.
///
/// Fixed rather than configurable. The root mirrors GitHub's own namespace, so
/// the depth is a property of that layout, not a preference — and fixing it is
/// what lets `repo clone` know where a clone belongs without being told.
pub const ROOT_DEPTH: usize = 2;

/// Category label for a repository whose owner is in no configured category.
pub const UNCATEGORIZED: &str = "-";

/// Owner categories, keyed by label. Insertion order is preserved so categories
/// colour and sort in the order they were written.
pub type Owners = IndexMap<String, Vec<String>>;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Config {
    /// The directory holding `<owner>/<repo>` checkouts, e.g.
    /// `~/dev/github.com`. Everything cloned lands here.
    pub root: Option<String>,
    /// Repositories outside [`Self::root`], listed individually. An escape
    /// hatch for checkouts that predate the single-root layout — not somewhere
    /// `clone` will ever write.
    pub extra: Vec<String>,
    /// Owner categories: a label to the GitHub owners it covers, one label to
    /// many owners, so `work` can span several orgs and colour as one.
    pub owners: Owners,
    pub ignore: Vec<String>,
    pub picker: PickerConfig,
    /// Editor launched by `scriv edit`, overriding `$VISUAL` and `$EDITOR`.
    pub editor: Option<String>,
}

impl Config {
    /// The config used when no config file exists. Repository discovery has
    /// nothing to search, but the known-files commands remain fully usable.
    pub fn empty() -> Self {
        Self {
            root: None,
            extra: Vec::new(),
            owners: Owners::new(),
            ignore: default_ignored_dirs(),
            picker: PickerConfig::default(),
            editor: None,
        }
    }

    /// The category `owner` belongs to, or `None` when it is in no category.
    ///
    /// Matched case-insensitively: GitHub treats `CapraLifecycle` and
    /// `capralifecycle` as the same owner, and a directory on disk may be
    /// spelled either way.
    pub fn category_of(&self, owner: &str) -> Option<&str> {
        self.owners
            .iter()
            .find(|(_, owners)| owners.iter().any(|o| o.eq_ignore_ascii_case(owner)))
            .map(|(label, _)| label.as_str())
    }

    /// Every owner named in the config, in category order — the owners worth
    /// offering first when there is an owner to choose.
    pub fn known_owners(&self) -> Vec<&str> {
        self.owners.values().flatten().map(String::as_str).collect()
    }
}

/// Pick the editor `scriv edit` launches: the `editor` config key first, then
/// `$VISUAL`, then `$EDITOR` — the order every other terminal tool uses, with
/// the config key on top so scriv can differ from the rest of the shell.
///
/// Blank and whitespace-only values count as unset: `EDITOR=""` is a common way
/// to say "no editor", and honouring it literally would try to spawn `""`.
pub fn resolve_editor(
    configured: Option<&str>,
    visual: Option<&str>,
    editor: Option<&str>,
) -> Option<String> {
    [configured, visual, editor]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|candidate| !candidate.is_empty())
        .map(str::to_string)
}

/// Split an editor command into program and arguments on whitespace, so
/// `EDITOR="code -w"` spawns `code` with `-w` rather than looking for a program
/// with a space in its name.
///
/// Whitespace is the whole of the syntax — no quoting, no escapes — which is
/// what git's own `core.editor` fallback does for the simple cases and covers
/// every editor invocation short of an embedded shell command.
pub fn split_editor(command: &str) -> Vec<String> {
    command.split_whitespace().map(str::to_string).collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PathEntry {
    pub path: String,
    #[serde(default)]
    pub depth: usize,
}

/// Settings for the built-in fuzzy picker (skim).
///
/// The picker is compiled in — there is no external `fzf` dependency.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct PickerConfig {
    /// Finder height, e.g. `"50%"` or `"20"`. Passed through to skim.
    pub height: String,
    /// How repository paths are rendered in `repo pick`. See [`RepoDisplay`].
    pub display: RepoDisplay,
    /// Whether the picker shows a preview pane for the highlighted row.
    pub preview: bool,
    /// Preview pane layout in skim's syntax, e.g. `"right:50%"`, `"down:40%"`,
    /// or `"right:50%:hidden"` to start collapsed.
    pub preview_window: String,
}

/// How `repo pick` renders each repository's path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RepoDisplay {
    /// Path relative to the search root it was found under, so the shared base
    /// (named by the group) is not repeated on every row. The default.
    #[default]
    Relative,
    /// Absolute path with the home directory collapsed to `~`.
    Tilde,
    /// The full absolute path.
    Full,
}

impl Default for PickerConfig {
    fn default() -> Self {
        Self {
            height: "50%".to_string(),
            display: RepoDisplay::default(),
            preview: true,
            preview_window: "right:50%".to_string(),
        }
    }
}

/// The serialized shape of `config.toml`.
#[derive(Deserialize)]
struct RawToml {
    root: Option<String>,
    #[serde(default)]
    extra: Vec<String>,
    #[serde(default)]
    owners: Owners,
    ignore: Option<Vec<String>>,
    #[serde(default)]
    picker: PickerConfig,
    editor: Option<String>,
    /// Only ever present in a pre-root config, and only so it can be detected
    /// and turned into migration advice. See [`migration_hint`].
    #[serde(default)]
    paths: Option<LegacyPaths>,
}

/// The `paths` key from the config format that preceded [`Config::root`]:
/// either grouped (`[[paths.work]]`) or a bare list (`[[paths]]`).
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum LegacyPaths {
    Grouped(IndexMap<String, Vec<PathEntry>>),
    Flat(Vec<PathEntry>),
}

impl LegacyPaths {
    /// Flatten to (category, entry) pairs; a bare list has no category.
    fn entries(&self) -> Vec<(Option<&str>, &PathEntry)> {
        match self {
            Self::Grouped(groups) => groups
                .iter()
                .flat_map(|(g, es)| es.iter().map(move |e| (Some(g.as_str()), e)))
                .collect(),
            Self::Flat(entries) => entries.iter().map(|e| (None, e)).collect(),
        }
    }
}

/// Turn an old `paths` config into the config that replaces it, as text the
/// user can paste.
///
/// The old format encoded the owner in the path — `~/dev/github.com/joakimen`
/// at depth 1 — which is exactly the two facts the new format wants stated
/// separately: one root, and which owners fall in which category. An entry that
/// does not fit that shape (`~/bin` at depth 0) becomes an `extra` path, since
/// that is what `extra` is for.
pub fn migration_hint(paths: &LegacyPaths) -> String {
    let mut roots: IndexMap<String, usize> = IndexMap::new();
    let mut owners: Owners = Owners::new();
    let mut extra: Vec<String> = Vec::new();

    for (category, entry) in paths.entries() {
        // `<root>/<owner>` at depth 1 is the layout the new root replaces.
        let split = entry.path.rsplit_once('/');
        match (entry.depth, split) {
            (1, Some((root, owner))) if !root.is_empty() && !owner.is_empty() => {
                *roots.entry(root.to_string()).or_insert(0) += 1;
                owners
                    .entry(category.unwrap_or("personal").to_string())
                    .or_default()
                    .push(owner.to_string());
            }
            // Anything else keeps working, just listed one path at a time.
            _ => extra.push(entry.path.clone()),
        }
    }

    let root = roots
        .into_iter()
        .max_by_key(|&(_, n)| n)
        .map(|(r, _)| r)
        .unwrap_or_else(|| "~/dev/github.com".to_string());

    let mut out = format!("root = {root:?}\n");
    if !extra.is_empty() {
        let list: Vec<String> = extra.iter().map(|p| format!("{p:?}")).collect();
        out.push_str(&format!("extra = [{}]\n", list.join(", ")));
    }
    if !owners.is_empty() {
        out.push_str("\n[owners]\n");
        for (category, list) in &owners {
            let list: Vec<String> = list.iter().map(|o| format!("{o:?}")).collect();
            out.push_str(&format!("{category} = [{}]\n", list.join(", ")));
        }
    }
    out
}

/// The error raised for a config still written in the old `paths` format.
fn legacy_error(paths: &LegacyPaths) -> anyhow::Error {
    anyhow::anyhow!(
        "this config uses the old `paths` format, which scriv no longer reads.\n\n\
         Repositories now live under one root as `<owner>/<repo>`, and categories \
         label owners rather than paths. Rewrite the `paths` section as:\n\n{}\n\
         `extra` is for repositories outside the root; drop the key if there are none.",
        indent(&migration_hint(paths))
    )
}

fn indent(text: &str) -> String {
    text.lines()
        .map(|l| {
            if l.is_empty() {
                l.to_string()
            } else {
                format!("    {l}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Parse `config.toml` contents.
fn parse_toml(data: &str) -> Result<Config> {
    let raw: RawToml = toml::from_str(data).context("parsing configuration file")?;
    if let Some(paths) = &raw.paths {
        return Err(legacy_error(paths));
    }
    Ok(Config {
        root: raw.root,
        extra: raw.extra,
        owners: raw.owners,
        ignore: raw.ignore.unwrap_or_else(default_ignored_dirs),
        picker: raw.picker,
        editor: raw.editor,
    })
}

/// The serialized shape of the legacy `config.json`, which only ever had the
/// old flat `paths` list.
#[derive(Deserialize)]
struct RawJson {
    #[serde(default)]
    paths: Vec<PathEntry>,
}

/// Legacy `config.json` predates the root layout entirely, so it gets the same
/// migration advice — pointed at a `config.toml`, which is what it should have
/// become long before now.
fn parse_json(data: &str) -> Result<Config> {
    let raw: RawJson = serde_json::from_str(data).context("parsing configuration file")?;
    Err(legacy_error(&LegacyPaths::Flat(raw.paths)))
}

/// Read and parse the config file at `path`, dispatching on its extension.
///
/// A missing file is not an error: it yields [`Config::empty`], so the
/// known-files commands work before any config has been written.
pub fn load_config(path: &Path) -> Result<Config> {
    let data = match std::fs::read_to_string(path) {
        Ok(data) => data,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Config::empty()),
        Err(e) => {
            return Err(e)
                .with_context(|| format!("reading configuration file: {}", path.display()));
        }
    };

    let parsed = match path.extension().and_then(|e| e.to_str()) {
        Some("json") => parse_json(&data),
        _ => parse_toml(&data),
    };
    parsed.with_context(|| format!("in {}", path.display()))
}

/// The directory holding `config.toml` and `files`:
/// `$XDG_CONFIG_HOME/scriv`, falling back to `~/.config/scriv`.
fn config_dir(xdg_env: Option<&str>, home: &Path) -> PathBuf {
    xdg_base(xdg_env, home).join("scriv")
}

/// Resolve the config file path by precedence:
/// explicit `flag` > `SCRIV_CONFIG` > `config.toml` > legacy `config.json`.
///
/// `exists` reports whether a candidate path is present, passed in so the
/// precedence rules stay testable without touching disk. When neither default
/// candidate exists, the TOML path is returned so callers and error messages
/// name the file the user is expected to create.
pub fn resolve_config_path(
    flag: Option<&str>,
    scriv_env: Option<&str>,
    xdg_env: Option<&str>,
    home: &Path,
    exists: impl Fn(&Path) -> bool,
) -> PathBuf {
    if let Some(flag) = flag.filter(|s| !s.is_empty()) {
        return expand_home_dir(flag, home);
    }
    if let Some(env) = scriv_env.filter(|s| !s.is_empty()) {
        return expand_home_dir(env, home);
    }

    let dir = config_dir(xdg_env, home);
    let toml_path = dir.join("config.toml");
    if exists(&toml_path) {
        return toml_path;
    }
    let json_path = dir.join("config.json");
    if exists(&json_path) {
        return json_path;
    }
    toml_path
}

/// The known-files list, kept beside the config file it belongs to.
pub fn files_path(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .join("files")
}

/// The standalone `kf` tool's config location, read once to migrate its list.
pub fn legacy_kf_path(xdg_env: Option<&str>, home: &Path) -> PathBuf {
    xdg_base(xdg_env, home).join("kf").join("config")
}

fn xdg_base(xdg_env: Option<&str>, home: &Path) -> PathBuf {
    match xdg_env.filter(|s| !s.is_empty()) {
        Some(xdg) => PathBuf::from(xdg),
        None => home.join(".config"),
    }
}

fn default_ignored_dirs() -> Vec<String> {
    DEFAULT_IGNORED_DIRS.iter().map(|s| s.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn never(_: &Path) -> bool {
        false
    }

    fn entry(path: &str, depth: usize) -> PathEntry {
        PathEntry {
            path: path.into(),
            depth,
        }
    }

    #[test]
    fn parses_root_extra_and_owners() {
        let cfg = parse_toml(
            r#"
root = "~/dev/github.com"
extra = ["~/bin"]

[owners]
personal = ["joakimen"]
work = ["capralifecycle", "nsbno"]
"#,
        )
        .unwrap();
        assert_eq!(cfg.root.as_deref(), Some("~/dev/github.com"));
        assert_eq!(cfg.extra, vec!["~/bin".to_string()]);
        // Config order, not alphabetical: it drives colour assignment.
        assert_eq!(
            cfg.owners.keys().collect::<Vec<_>>(),
            vec!["personal", "work"]
        );
    }

    /// One category covers many owners, which is the whole point: everything
    /// touched for work colours alike however many orgs it spans.
    #[test]
    fn a_category_spans_several_owners() {
        let cfg = parse_toml("[owners]\nwork = [\"capralifecycle\", \"nsbno\"]\n").unwrap();
        assert_eq!(cfg.category_of("capralifecycle"), Some("work"));
        assert_eq!(cfg.category_of("nsbno"), Some("work"));
        assert_eq!(cfg.category_of("joakimen"), None);
    }

    /// GitHub owners are case-insensitive, and a directory on disk may be
    /// spelled differently from the config.
    #[test]
    fn owner_lookup_ignores_case() {
        let cfg = parse_toml("[owners]\nwork = [\"CapraLifecycle\"]\n").unwrap();
        assert_eq!(cfg.category_of("capralifecycle"), Some("work"));
        assert_eq!(cfg.category_of("CAPRALIFECYCLE"), Some("work"));
    }

    #[test]
    fn empty_toml_has_no_root() {
        let cfg = parse_toml("").unwrap();
        assert_eq!(cfg.root, None);
        assert!(cfg.extra.is_empty());
        assert!(cfg.owners.is_empty());
        assert_eq!(cfg.ignore, default_ignored_dirs());
    }

    /// The old format is refused rather than half-understood — but the error
    /// has to be worth reading, so it derives the replacement from what was
    /// there. `<root>/<owner>` at depth 1 is exactly the two facts the new
    /// format states separately.
    #[test]
    fn legacy_paths_config_is_rejected_with_a_migration() {
        let err = parse_toml(
            r#"
[[paths.personal]]
path = "~/dev/github.com/joakimen"
depth = 1

[[paths.personal]]
path = "~/bin"
depth = 0

[[paths.work]]
path = "~/dev/github.com/capralifecycle"
depth = 1
"#,
        )
        .unwrap_err()
        .to_string();

        assert!(err.contains("old `paths` format"), "{err}");
        assert!(err.contains(r#"root = "~/dev/github.com""#), "{err}");
        assert!(err.contains(r#"personal = ["joakimen"]"#), "{err}");
        assert!(err.contains(r#"work = ["capralifecycle"]"#), "{err}");
        // ~/bin does not fit <root>/<owner>, so it becomes an extra path.
        assert!(err.contains(r#"extra = ["~/bin"]"#), "{err}");
    }

    /// The root is the one shared by most entries, not whichever came first.
    #[test]
    fn migration_picks_the_most_common_root() {
        let paths = LegacyPaths::Grouped(
            [
                (
                    "a".to_string(),
                    vec![entry("~/odd/one-off", 1), entry("~/dev/github.com/x", 1)],
                ),
                ("b".to_string(), vec![entry("~/dev/github.com/y", 1)]),
            ]
            .into_iter()
            .collect(),
        );
        assert!(
            migration_hint(&paths).contains(r#"root = "~/dev/github.com""#),
            "{}",
            migration_hint(&paths)
        );
    }

    /// A flat legacy list has no categories to carry over, but still needs a
    /// root and must not lose any path.
    #[test]
    fn migration_handles_an_ungrouped_list() {
        let hint = migration_hint(&LegacyPaths::Flat(vec![
            entry("~/dev/github.com/joakimen", 1),
            entry("~/bin", 0),
        ]));
        assert!(hint.contains(r#"root = "~/dev/github.com""#), "{hint}");
        assert!(hint.contains(r#"extra = ["~/bin"]"#), "{hint}");
    }

    /// Legacy JSON predates the root layout too; it gets the same advice
    /// instead of being silently half-read.
    #[test]
    fn legacy_json_is_rejected_with_a_migration() {
        let err = parse_json(r#"{ "paths": [{"path": "~/dev/github.com/joakimen", "depth": 1}] }"#)
            .unwrap_err()
            .to_string();
        assert!(err.contains("old `paths` format"), "{err}");
        assert!(err.contains(r#"root = "~/dev/github.com""#), "{err}");
    }

    #[test]
    fn parses_picker_height() {
        let cfg = parse_toml("[picker]\nheight = \"30%\"\n").unwrap();
        assert_eq!(cfg.picker.height, "30%");
    }

    #[test]
    fn parses_picker_display_mode() {
        let cfg = parse_toml("[picker]\ndisplay = \"tilde\"\n").unwrap();
        assert_eq!(cfg.picker.display, RepoDisplay::Tilde);
    }

    #[test]
    fn picker_display_defaults_to_relative() {
        assert_eq!(
            parse_toml("").unwrap().picker.display,
            RepoDisplay::Relative
        );
    }

    #[test]
    fn parses_preview_settings() {
        let cfg = parse_toml("[picker]\npreview = false\npreview_window = \"down:40%\"\n").unwrap();
        assert!(!cfg.picker.preview);
        assert_eq!(cfg.picker.preview_window, "down:40%");
    }

    #[test]
    fn preview_defaults_to_on_at_the_right() {
        let cfg = parse_toml("").unwrap();
        assert!(cfg.picker.preview);
        assert_eq!(cfg.picker.preview_window, "right:50%");
    }

    #[test]
    fn picker_height_defaults_to_50pct() {
        assert_eq!(parse_toml("").unwrap().picker.height, "50%");
        // A `[picker]` table with no height still gets the default.
        assert_eq!(parse_toml("[picker]\n").unwrap().picker.height, "50%");
    }

    /// Unknown picker keys from older configs (e.g. `backend`) are ignored, not
    /// an error.
    #[test]
    fn picker_ignores_unknown_keys() {
        let cfg = parse_toml("[picker]\nbackend = \"fzf\"\nheight = \"10\"\n").unwrap();
        assert_eq!(cfg.picker.height, "10");
    }

    #[test]
    fn toml_preserves_explicit_empty_ignore() {
        assert!(parse_toml("ignore = []").unwrap().ignore.is_empty());
    }

    #[test]
    fn rejects_invalid_toml() {
        assert!(parse_toml("[[paths]\npath =").is_err());
    }

    #[test]
    fn rejects_invalid_json() {
        assert!(parse_json("{not json").is_err());
    }

    #[test]
    fn flag_takes_precedence() {
        let home = Path::new("/home/user");
        let got = resolve_config_path(
            Some("~/custom.toml"),
            Some("/env.toml"),
            Some("/xdg"),
            home,
            never,
        );
        assert_eq!(got, home.join("custom.toml"));
    }

    #[test]
    fn env_beats_xdg_and_home() {
        let home = Path::new("/home/user");
        let got = resolve_config_path(None, Some("/env.toml"), Some("/xdg"), home, never);
        assert_eq!(got, PathBuf::from("/env.toml"));
    }

    #[test]
    fn xdg_beats_home() {
        let home = Path::new("/home/user");
        let got = resolve_config_path(None, None, Some("/xdg"), home, never);
        assert_eq!(got, PathBuf::from("/xdg/scriv/config.toml"));
    }

    #[test]
    fn falls_back_to_home_config() {
        let home = Path::new("/home/user");
        let got = resolve_config_path(None, None, None, home, never);
        assert_eq!(got, home.join(".config/scriv/config.toml"));
    }

    #[test]
    fn prefers_toml_over_legacy_json() {
        let home = Path::new("/home/user");
        let got = resolve_config_path(None, None, None, home, |_| true);
        assert_eq!(got, home.join(".config/scriv/config.toml"));
    }

    #[test]
    fn uses_legacy_json_when_only_it_exists() {
        let home = Path::new("/home/user");
        let json = home.join(".config/scriv/config.json");
        let got = resolve_config_path(None, None, None, home, |p| p == json);
        assert_eq!(got, json);
    }

    #[test]
    fn ignores_empty_env_values() {
        let home = Path::new("/home/user");
        let got = resolve_config_path(Some(""), Some(""), Some(""), home, never);
        assert_eq!(got, home.join(".config/scriv/config.toml"));
    }

    #[test]
    fn files_list_sits_beside_the_config() {
        assert_eq!(
            files_path(Path::new("/home/user/.config/scriv/config.toml")),
            PathBuf::from("/home/user/.config/scriv/files")
        );
    }

    #[test]
    fn files_list_handles_bare_config_name() {
        assert_eq!(
            files_path(Path::new("config.toml")),
            PathBuf::from("./files")
        );
    }

    #[test]
    fn legacy_kf_path_follows_xdg() {
        let home = Path::new("/home/user");
        assert_eq!(legacy_kf_path(None, home), home.join(".config/kf/config"));
        assert_eq!(
            legacy_kf_path(Some("/xdg"), home),
            PathBuf::from("/xdg/kf/config")
        );
    }

    #[test]
    fn parses_editor() {
        assert_eq!(
            parse_toml("editor = \"hx\"\n").unwrap().editor.as_deref(),
            Some("hx")
        );
        assert_eq!(parse_toml("").unwrap().editor, None);
    }

    #[test]
    fn editor_precedence_prefers_config_then_visual() {
        assert_eq!(
            resolve_editor(Some("hx"), Some("code"), Some("vi")).as_deref(),
            Some("hx")
        );
        assert_eq!(
            resolve_editor(None, Some("code"), Some("vi")).as_deref(),
            Some("code")
        );
        assert_eq!(
            resolve_editor(None, None, Some("vi")).as_deref(),
            Some("vi")
        );
        assert_eq!(resolve_editor(None, None, None), None);
    }

    /// `EDITOR=""` means "no editor", not a program named "".
    #[test]
    fn editor_ignores_blank_values() {
        assert_eq!(
            resolve_editor(None, Some("  "), Some("vi")).as_deref(),
            Some("vi")
        );
        assert_eq!(resolve_editor(Some(""), None, None), None);
        // A value that is only padded still resolves, trimmed.
        assert_eq!(
            resolve_editor(Some(" hx "), None, None).as_deref(),
            Some("hx")
        );
    }

    #[test]
    fn splits_editor_into_program_and_args() {
        assert_eq!(split_editor("nvim"), vec!["nvim"]);
        assert_eq!(split_editor("code -w"), vec!["code", "-w"]);
        assert_eq!(split_editor("  nvim   -p  "), vec!["nvim", "-p"]);
    }
}
