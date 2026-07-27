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
/// Group name assigned to paths from an ungrouped (flat or legacy) config.
pub const DEFAULT_GROUP: &str = "default";

/// Search paths keyed by group name. Insertion order is preserved so groups
/// display in the order the user wrote them.
pub type Groups = IndexMap<String, Vec<PathEntry>>;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Config {
    /// Search paths, keyed by the group label repos under them belong to.
    pub paths: Groups,
    pub ignore: Vec<String>,
    pub picker: PickerConfig,
}

impl Config {
    /// The config used when no config file exists. Repository discovery has
    /// nothing to search, but the known-files commands remain fully usable.
    pub fn empty() -> Self {
        Self {
            paths: Groups::new(),
            ignore: default_ignored_dirs(),
            picker: PickerConfig::default(),
        }
    }
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
        }
    }
}

/// The serialized shape of `config.toml`.
#[derive(Deserialize)]
struct RawToml {
    #[serde(default)]
    paths: RawPaths,
    ignore: Option<Vec<String>>,
    #[serde(default)]
    picker: PickerConfig,
}

/// `paths` accepts two shapes. The grouped form keys entries by a group label
/// (`[[paths.work]]`); the flat form is a bare list (`[[paths]]`) whose entries
/// land in the [`DEFAULT_GROUP`]. Untagged, so serde picks by structure.
#[derive(Deserialize)]
#[serde(untagged)]
enum RawPaths {
    Grouped(Groups),
    Flat(Vec<PathEntry>),
}

impl Default for RawPaths {
    fn default() -> Self {
        RawPaths::Grouped(Groups::new())
    }
}

impl RawPaths {
    fn into_groups(self) -> Groups {
        match self {
            RawPaths::Grouped(groups) => groups,
            RawPaths::Flat(entries) => flat_to_groups(entries),
        }
    }
}

/// The serialized shape of the legacy `config.json` (always a flat list).
///
/// `ignore` has been written both at the top level and nested under `settings`;
/// both are accepted, with the top-level key winning when both are present.
#[derive(Deserialize)]
struct RawJson {
    #[serde(default)]
    paths: Vec<PathEntry>,
    ignore: Option<Vec<String>>,
    #[serde(default)]
    settings: Option<JsonSettings>,
}

#[derive(Deserialize)]
struct JsonSettings {
    ignore: Option<Vec<String>>,
}

/// Wrap a flat entry list in the default group, or nothing when empty.
fn flat_to_groups(entries: Vec<PathEntry>) -> Groups {
    let mut groups = Groups::new();
    if !entries.is_empty() {
        groups.insert(DEFAULT_GROUP.to_string(), entries);
    }
    groups
}

/// Parse `config.toml` contents.
fn parse_toml(data: &str) -> Result<Config> {
    let raw: RawToml = toml::from_str(data).context("parsing configuration file")?;
    Ok(Config {
        paths: raw.paths.into_groups(),
        ignore: raw.ignore.unwrap_or_else(default_ignored_dirs),
        picker: raw.picker,
    })
}

/// Parse legacy `config.json` contents. A missing `ignore` key — at either
/// supported location — falls back to the default ignore list; an explicit
/// empty list is preserved.
fn parse_json(data: &str) -> Result<Config> {
    let raw: RawJson = serde_json::from_str(data).context("parsing configuration file")?;
    let ignore = raw
        .ignore
        .or_else(|| raw.settings.and_then(|s| s.ignore))
        .unwrap_or_else(default_ignored_dirs);
    Ok(Config {
        paths: flat_to_groups(raw.paths),
        ignore,
        picker: PickerConfig::default(),
    })
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
    fn parses_flat_toml_into_default_group() {
        let cfg = parse_toml(
            r#"
ignore = ["foo", "bar"]

[[paths]]
path = "~/dev"
depth = 2
"#,
        )
        .unwrap();
        assert_eq!(cfg.paths[DEFAULT_GROUP], vec![entry("~/dev", 2)]);
        assert_eq!(cfg.ignore, vec!["foo".to_string(), "bar".to_string()]);
    }

    #[test]
    fn parses_grouped_toml_preserving_order() {
        let cfg = parse_toml(
            r#"
[[paths.work]]
path = "~/work"
depth = 2

[[paths.personal]]
path = "~/dev"
depth = 1

[[paths.personal]]
path = "~/bin"
depth = 0
"#,
        )
        .unwrap();
        assert_eq!(
            cfg.paths.keys().collect::<Vec<_>>(),
            vec!["work", "personal"] // config order, not alphabetical
        );
        assert_eq!(cfg.paths["work"], vec![entry("~/work", 2)]);
        assert_eq!(
            cfg.paths["personal"],
            vec![entry("~/dev", 1), entry("~/bin", 0)]
        );
    }

    #[test]
    fn empty_toml_has_no_groups() {
        assert!(parse_toml("").unwrap().paths.is_empty());
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
    fn toml_applies_default_ignore_when_absent() {
        let cfg = parse_toml("[[paths]]\npath = \"~/dev\"\n").unwrap();
        assert_eq!(cfg.ignore, default_ignored_dirs());
        assert_eq!(cfg.paths[DEFAULT_GROUP][0].depth, 0);
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
    fn parses_legacy_json_into_default_group() {
        let cfg = parse_json(r#"{ "paths": [{"path": "~/dev", "depth": 2}], "ignore": ["foo"] }"#)
            .unwrap();
        assert_eq!(cfg.paths[DEFAULT_GROUP], vec![entry("~/dev", 2)]);
        assert_eq!(cfg.ignore, vec!["foo".to_string()]);
    }

    /// Regression: the nested form was silently dropped, leaving users on the
    /// default ignore list with no warning.
    #[test]
    fn json_accepts_ignore_nested_under_settings() {
        let cfg =
            parse_json(r#"{ "paths": [], "settings": { "ignore": ["node_modules", "target"] } }"#)
                .unwrap();
        assert_eq!(
            cfg.ignore,
            vec!["node_modules".to_string(), "target".to_string()]
        );
    }

    #[test]
    fn json_top_level_ignore_beats_nested() {
        let cfg =
            parse_json(r#"{ "ignore": ["top"], "settings": { "ignore": ["nested"] } }"#).unwrap();
        assert_eq!(cfg.ignore, vec!["top".to_string()]);
    }

    #[test]
    fn json_applies_default_ignore_when_absent() {
        let cfg = parse_json(r#"{ "paths": [{"path": "~/dev"}] }"#).unwrap();
        assert_eq!(cfg.ignore, default_ignored_dirs());
    }

    #[test]
    fn json_preserves_explicit_empty_ignore() {
        let cfg = parse_json(r#"{ "paths": [], "ignore": [] }"#).unwrap();
        assert!(cfg.ignore.is_empty());
    }

    #[test]
    fn json_preserves_explicit_empty_nested_ignore() {
        let cfg = parse_json(r#"{ "settings": { "ignore": [] } }"#).unwrap();
        assert!(cfg.ignore.is_empty());
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
}
