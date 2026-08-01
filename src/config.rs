//! Configuration model and loading. Parsing and path resolution are pure —
//! they take environment and home values as arguments — so the only I/O is the
//! file read in [`load_config`].
//!
//! Two files live side by side under the config directory:
//!
//! - `config.toml` — hand-edited settings, grouped by the command that reads
//!   them: `[repo]` for discovery and labelling, `[history]` for the shell
//!   history to search, `[selector]` for the finder every command shares. A
//!   legacy `config.json` is still read when no TOML file is present.
//! - `files` — the known-files list, rewritten programmatically by
//!   `scriv file add`/`rm`/`prune`. Kept separate so machine writes never
//!   clobber hand-written settings or comments.
//!
//! A key belongs in a command's table when exactly one command reads it;
//! anything genuinely shared stays at the top level. That is why `[selector]`
//! sits beside `[repo]` rather than inside it, and why `display` — a repo
//! path-rendering choice no other selector has — sits in `[repo]`.

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

/// How deep below [`RepoConfig::root`] a repository sits: `<root>/<owner>/<repo>`.
///
/// Fixed rather than configurable. The root mirrors GitHub's own namespace, so
/// the depth is a property of that layout, not a preference — and fixing it is
/// what lets `repo clone` know where a clone belongs without being told.
pub const ROOT_DEPTH: usize = 2;

/// Stand-in label for a repository whose owner carries no configured label.
pub const UNLABELLED: &str = "-";

/// Owner labels: a label to the GitHub owners it covers. Insertion order is
/// preserved so labels sort, and take their leftover colours, in the order they
/// were written — `work` and `personal` are coloured by name and do not depend
/// on it.
pub type Labels = IndexMap<String, Vec<String>>;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Config {
    pub repo: RepoConfig,
    pub history: HistoryConfig,
    pub selector: SelectorConfig,
}

impl Config {
    /// The config used when no config file exists. Repository discovery has
    /// nothing to search, but the known-files commands remain fully usable.
    pub fn empty() -> Self {
        Self::default()
    }
}

/// `[repo]` — where repositories live and how they are labelled and rendered.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct RepoConfig {
    /// The directory holding `<owner>/<repo>` checkouts, e.g.
    /// `~/dev/github.com`. Everything cloned lands here.
    pub root: Option<String>,
    /// Repositories outside [`Self::root`], listed individually. An escape
    /// hatch for checkouts that predate the single-root layout — not somewhere
    /// `clone` will ever write.
    pub extra: Vec<String>,
    /// Owner labels: a label to the GitHub owners it covers, one label to many
    /// owners, so `work` can span several orgs and colour as one.
    ///
    /// Written as an inline table — `labels = { work = ["acme"] }` — so it is
    /// an ordinary key of `[repo]` rather than a subtable header. A
    /// `[repo.labels]` header parses identically, but swallows every bare
    /// `[repo]` key written after it, which in a hand-edited file is a silent
    /// misconfiguration rather than an error.
    pub labels: Labels,
    /// Directory names skipped while searching for repositories.
    pub ignore: Vec<String>,
    /// How repository paths are rendered in `repo ls`/`sel`. See
    /// [`RepoDisplay`].
    pub display: RepoDisplay,
}

impl Default for RepoConfig {
    fn default() -> Self {
        Self {
            root: None,
            extra: Vec::new(),
            labels: Labels::new(),
            ignore: default_ignored_dirs(),
            display: RepoDisplay::default(),
        }
    }
}

impl RepoConfig {
    /// The label `owner` carries, or `None` when it has none.
    ///
    /// Matched case-insensitively: GitHub treats `CapraLifecycle` and
    /// `capralifecycle` as the same owner, and a directory on disk may be
    /// spelled either way.
    pub fn label_of(&self, owner: &str) -> Option<&str> {
        self.labels
            .iter()
            .find(|(_, owners)| owners.iter().any(|o| o.eq_ignore_ascii_case(owner)))
            .map(|(label, _)| label.as_str())
    }

    /// Every owner named in the config, in label order — the owners worth
    /// offering first when there is an owner to choose.
    pub fn known_owners(&self) -> Vec<&str> {
        self.labels.values().flatten().map(String::as_str).collect()
    }
}

/// `[history]` — which shell history `scriv history` searches.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(default)]
pub struct HistoryConfig {
    /// fish's history file, when it is not the default
    /// `$XDG_DATA_HOME/fish/fish_history`.
    ///
    /// The only reason to set this is a named session: `set -U fish_history
    /// work` makes fish read `work_history` instead, and it does not export
    /// that variable, so scriv has no way to find out on its own.
    pub file: Option<String>,
}

/// Select the editor `scriv edit` launches: `$VISUAL`, then `$EDITOR` — the order
/// every other terminal tool uses.
///
/// There is deliberately no config key on top. An editor is a property of the
/// shell session, already stated once where every other tool reads it, and a
/// third place to set it is a third place to forget it is set.
///
/// Blank and whitespace-only values count as unset: `EDITOR=""` is a common way
/// to say "no editor", and honouring it literally would try to spawn `""`.
pub fn resolve_editor(visual: Option<&str>, editor: Option<&str>) -> Option<String> {
    [visual, editor]
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

/// Settings for the built-in fuzzy selector (skim), shared by every command that
/// opens one.
///
/// The selector is compiled in — there is no external `fzf` dependency.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectorConfig {
    /// Finder height, e.g. `"50%"` or `"20"`. Passed through to skim.
    pub height: String,
    /// Whether the selector shows a preview pane for the highlighted row.
    pub preview: bool,
    /// Preview pane layout in skim's syntax, e.g. `"right:50%"`, `"down:40%"`,
    /// or `"right:50%:hidden"` to start collapsed.
    pub preview_window: String,
}

/// How `repo ls`/`sel` renders each repository's path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RepoDisplay {
    /// Path relative to the search root it was found under, so the shared base
    /// is not repeated on every row. The default.
    #[default]
    Relative,
    /// Absolute path with the home directory collapsed to `~`.
    Tilde,
    /// The full absolute path.
    Full,
}

impl RepoDisplay {
    /// The name this mode is written under in the config file.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Relative => "relative",
            Self::Tilde => "tilde",
            Self::Full => "full",
        }
    }
}

impl Default for SelectorConfig {
    fn default() -> Self {
        Self {
            height: "50%".to_string(),
            preview: true,
            preview_window: "right:50%".to_string(),
        }
    }
}

/// The serialized shape of `config.toml`.
///
/// Every key of the layout that preceded `[repo]` is still spelled out here, at
/// the top level, so an old config can be recognised and explained rather than
/// half-read: serde ignores what it does not know, and a silently ignored
/// `root` would look like a config that simply found no repositories.
#[derive(Deserialize)]
struct RawToml {
    #[serde(default)]
    repo: RepoConfig,
    #[serde(default)]
    history: HistoryConfig,
    #[serde(default)]
    selector: RawSelector,

    // Superseded top-level keys, present for detection only.
    root: Option<String>,
    extra: Option<Vec<String>>,
    owners: Option<Labels>,
    ignore: Option<Vec<String>>,
    editor: Option<String>,
    /// The `paths` key from the layout before that. See [`migration_hint`].
    paths: Option<LegacyPaths>,
    /// `[picker]`, renamed to `[selector]`. Present for detection only, so a
    /// config written under the old name is explained rather than silently
    /// ignored back to the defaults.
    picker: Option<RawSelector>,
}

/// `[selector]` as written, including the `display` key that moved to `[repo]`.
///
/// Every field is optional so an absent one takes [`SelectorConfig`]'s default
/// while an explicitly written one is preserved.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawSelector {
    height: Option<String>,
    preview: Option<bool>,
    preview_window: Option<String>,
    /// Superseded by `[repo] display`, present for detection only.
    display: Option<RepoDisplay>,
}

impl From<RawSelector> for SelectorConfig {
    fn from(raw: RawSelector) -> Self {
        let default = Self::default();
        Self {
            height: raw.height.unwrap_or(default.height),
            preview: raw.preview.unwrap_or(default.preview),
            preview_window: raw.preview_window.unwrap_or(default.preview_window),
        }
    }
}

/// A config written in the flat layout that preceded `[repo]`, gathered so the
/// error can hand back the whole replacement rather than one key at a time.
struct FlatLayout {
    root: Option<String>,
    extra: Vec<String>,
    labels: Labels,
    ignore: Option<Vec<String>>,
    display: Option<RepoDisplay>,
    editor: bool,
}

impl RawToml {
    /// The superseded keys this config uses, or `None` if it uses none.
    fn flat_layout(&self) -> Option<FlatLayout> {
        let flat = FlatLayout {
            root: self.root.clone(),
            extra: self.extra.clone().unwrap_or_default(),
            labels: self.owners.clone().unwrap_or_default(),
            ignore: self.ignore.clone(),
            display: self
                .selector
                .display
                .or_else(|| self.picker.as_ref().and_then(|p| p.display)),
            editor: self.editor.is_some(),
        };
        let used = flat.root.is_some()
            || !flat.extra.is_empty()
            || !flat.labels.is_empty()
            || flat.ignore.is_some()
            || flat.display.is_some()
            || flat.editor;
        used.then_some(flat)
    }
}

/// The `paths` key from the config format that preceded `root`: either grouped
/// (`[[paths.work]]`) or a bare list (`[[paths]]`).
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum LegacyPaths {
    Grouped(IndexMap<String, Vec<PathEntry>>),
    Flat(Vec<PathEntry>),
}

impl LegacyPaths {
    /// Flatten to (label, entry) pairs; a bare list has no label.
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

/// Render a `[repo]` section, as text the user can paste.
///
/// `labels` is written as an inline table rather than a `[repo.labels]` header
/// precisely because this is advice being pasted into a file that already has
/// other keys: a header would capture whatever follows it.
fn render_repo_section(
    root: &str,
    extra: &[String],
    ignore: Option<&[String]>,
    display: Option<RepoDisplay>,
    labels: &Labels,
) -> String {
    let list = |items: &[String]| -> String {
        items
            .iter()
            .map(|i| format!("{i:?}"))
            .collect::<Vec<_>>()
            .join(", ")
    };

    let mut out = format!("[repo]\nroot = {root:?}\n");
    if !extra.is_empty() {
        out.push_str(&format!("extra = [{}]\n", list(extra)));
    }
    if let Some(ignore) = ignore {
        out.push_str(&format!("ignore = [{}]\n", list(ignore)));
    }
    if let Some(display) = display {
        out.push_str(&format!("display = {:?}\n", display.as_str()));
    }
    if !labels.is_empty() {
        let pairs: Vec<String> = labels
            .iter()
            .map(|(label, owners)| format!("{label} = [{}]", list(owners)))
            .collect();
        out.push_str(&format!("labels = {{ {} }}\n", pairs.join(", ")));
    }
    out
}

/// Turn an old `paths` config into the config that replaces it, as text the
/// user can paste.
///
/// The old format encoded the owner in the path — `~/dev/github.com/joakimen`
/// at depth 1 — which is exactly the two facts the new format wants stated
/// separately: one root, and which owners carry which label. An entry that does
/// not fit that shape (`~/bin` at depth 0) becomes an `extra` path, since that
/// is what `extra` is for.
pub fn migration_hint(paths: &LegacyPaths) -> String {
    let mut roots: IndexMap<String, usize> = IndexMap::new();
    let mut labels: Labels = Labels::new();
    let mut extra: Vec<String> = Vec::new();

    for (label, entry) in paths.entries() {
        // `<root>/<owner>` at depth 1 is the layout the new root replaces.
        let split = entry.path.rsplit_once('/');
        match (entry.depth, split) {
            (1, Some((root, owner))) if !root.is_empty() && !owner.is_empty() => {
                *roots.entry(root.to_string()).or_insert(0) += 1;
                labels
                    .entry(label.unwrap_or("personal").to_string())
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

    render_repo_section(&root, &extra, None, None, &labels)
}

/// The error raised for a config still written in the old `paths` format.
fn legacy_paths_error(paths: &LegacyPaths) -> anyhow::Error {
    anyhow::anyhow!(
        "this config uses the old `paths` format, which scriv no longer reads.\n\n\
         Repositories now live under one root as `<owner>/<repo>`, and labels name \
         owners rather than paths. Rewrite the `paths` section as:\n\n{}\n\n\
         `extra` is for repositories outside the root; drop the key if there are none.",
        indent(&migration_hint(paths))
    )
}

impl FlatLayout {
    /// The `[repo]` section that replaces these keys, as text the user can
    /// paste.
    fn replacement(&self) -> String {
        render_repo_section(
            self.root.as_deref().unwrap_or("~/dev/github.com"),
            &self.extra,
            self.ignore.as_deref(),
            self.display,
            &self.labels,
        )
    }
}

/// The error raised for a config still using the flat, ungrouped keys.
fn legacy_flat_error(flat: &FlatLayout) -> anyhow::Error {
    let replacement = flat.replacement();

    let mut note = String::new();
    if flat.editor {
        note.push_str(
            "\n\n`editor` is gone: scriv uses $VISUAL, then $EDITOR, like every \
             other terminal tool.",
        );
    }

    anyhow::anyhow!(
        "this config uses the old flat layout, which scriv no longer reads.\n\n\
         Settings are now grouped by the command that reads them: repository \
         discovery under `[repo]`, and `owners` renamed to `labels`. Rewrite as:\n\n{}\n\n\
         `[selector]` keeps `height`, `preview` and `preview_window`.{}",
        indent(&replacement),
        note
    )
}

/// The `[selector]` table replacing a `[picker]` one, carrying over the keys
/// that were actually written — an absent key means the default, and writing
/// one out would freeze today's default into the user's file.
fn renamed_picker_table(picker: &RawSelector) -> String {
    let mut table = String::from("[selector]\n");
    if let Some(height) = &picker.height {
        table.push_str(&format!("height = {height:?}\n"));
    }
    if let Some(preview) = picker.preview {
        table.push_str(&format!("preview = {preview}\n"));
    }
    if let Some(window) = &picker.preview_window {
        table.push_str(&format!("preview_window = {window:?}\n"));
    }
    table
}

/// The error raised for a config whose finder settings are still under
/// `[picker]`.
///
/// Silently reading them under both names would leave the old spelling working
/// forever, and silently ignoring them would reset a customised height to the
/// default without a word — so the table is named back to the user with the
/// keys they wrote already in it.
fn renamed_picker_error(picker: &RawSelector) -> anyhow::Error {
    let table = renamed_picker_table(picker);

    let note = if picker.display.is_some() {
        "\n\n`display` is not one of its keys: it moved to `[repo]`."
    } else {
        ""
    };

    anyhow::anyhow!(
        "this config has a `[picker]` table, which scriv no longer reads.\n\n\
         The finder is the selector now — `scriv <group> sel` — and its settings \
         are spelled the same way. Rewrite as:\n\n{}{}",
        indent(&table),
        note
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
        return Err(legacy_paths_error(paths));
    }
    if let Some(picker) = &raw.picker {
        return Err(renamed_picker_error(picker));
    }
    if let Some(flat) = raw.flat_layout() {
        return Err(legacy_flat_error(&flat));
    }
    Ok(Config {
        repo: raw.repo,
        history: raw.history,
        selector: raw.selector.into(),
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
    Err(legacy_paths_error(&LegacyPaths::Flat(raw.paths)))
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
    fn parses_root_extra_and_labels() {
        let cfg = parse_toml(
            r#"
[repo]
root = "~/dev/github.com"
extra = ["~/bin"]
labels = { personal = ["joakimen"], work = ["capralifecycle", "nsbno"] }
"#,
        )
        .unwrap();
        assert_eq!(cfg.repo.root.as_deref(), Some("~/dev/github.com"));
        assert_eq!(cfg.repo.extra, vec!["~/bin".to_string()]);
        // Config order, not alphabetical: it drives colour assignment.
        assert_eq!(
            cfg.repo.labels.keys().collect::<Vec<_>>(),
            vec!["personal", "work"]
        );
    }

    /// The inline table is what the template teaches, because a `[repo.labels]`
    /// header swallows any bare `[repo]` key written after it. Both spellings
    /// have to keep working for anyone who prefers the header.
    #[test]
    fn labels_accept_a_subtable_header_too() {
        let cfg = parse_toml(
            r#"
[repo]
root = "~/src"

[repo.labels]
work = ["acme"]
"#,
        )
        .unwrap();
        assert_eq!(cfg.repo.root.as_deref(), Some("~/src"));
        assert_eq!(cfg.repo.label_of("acme"), Some("work"));
    }

    /// A key written after an inline `labels` is still a `[repo]` key — the
    /// ordering fragility the inline form exists to avoid.
    #[test]
    fn keys_after_inline_labels_stay_repo_keys() {
        let cfg = parse_toml(
            r#"
[repo]
labels = { work = ["acme"] }
ignore = ["node_modules"]
"#,
        )
        .unwrap();
        assert_eq!(cfg.repo.ignore, vec!["node_modules".to_string()]);
        assert_eq!(cfg.repo.label_of("acme"), Some("work"));
    }

    /// One label covers many owners, which is the whole point: everything
    /// touched for work colours alike however many orgs it spans.
    #[test]
    fn a_label_spans_several_owners() {
        let cfg =
            parse_toml("[repo]\nlabels = { work = [\"capralifecycle\", \"nsbno\"] }\n").unwrap();
        assert_eq!(cfg.repo.label_of("capralifecycle"), Some("work"));
        assert_eq!(cfg.repo.label_of("nsbno"), Some("work"));
        assert_eq!(cfg.repo.label_of("joakimen"), None);
    }

    /// GitHub owners are case-insensitive, and a directory on disk may be
    /// spelled differently from the config.
    #[test]
    fn owner_lookup_ignores_case() {
        let cfg = parse_toml("[repo]\nlabels = { work = [\"CapraLifecycle\"] }\n").unwrap();
        assert_eq!(cfg.repo.label_of("capralifecycle"), Some("work"));
        assert_eq!(cfg.repo.label_of("CAPRALIFECYCLE"), Some("work"));
    }

    #[test]
    fn known_owners_follow_label_order() {
        let cfg = parse_toml(
            "[repo]\nlabels = { work = [\"acme\", \"acme-labs\"], personal = [\"me\"] }\n",
        )
        .unwrap();
        assert_eq!(cfg.repo.known_owners(), vec!["acme", "acme-labs", "me"]);
    }

    #[test]
    fn empty_toml_has_no_root() {
        let cfg = parse_toml("").unwrap();
        assert_eq!(cfg.repo.root, None);
        assert!(cfg.repo.extra.is_empty());
        assert!(cfg.repo.labels.is_empty());
        assert_eq!(cfg.repo.ignore, default_ignored_dirs());
        assert_eq!(cfg.repo.display, RepoDisplay::Relative);
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
        assert!(err.contains("[repo]"), "{err}");
        assert!(err.contains(r#"root = "~/dev/github.com""#), "{err}");
        assert!(
            err.contains(r#"labels = { personal = ["joakimen"], work = ["capralifecycle"] }"#),
            "{err}"
        );
        // ~/bin does not fit <root>/<owner>, so it becomes an extra path.
        assert!(err.contains(r#"extra = ["~/bin"]"#), "{err}");
    }

    /// The root is the one shared by most entries, not whichever came first.
    #[test]
    fn migration_selects_the_most_common_root() {
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

    /// A flat legacy list has no labels to carry over, but still needs a root
    /// and must not lose any path.
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

    /// The flat layout is refused with its own replacement written out, for the
    /// same reason: serde would otherwise ignore every superseded key and the
    /// config would look like one that simply found nothing.
    #[test]
    fn flat_layout_is_rejected_with_a_migration() {
        let err = parse_toml(
            r#"
root = "~/dev/github.com"
extra = ["~/bin"]
ignore = ["target"]
editor = "nvim"

[owners]
work = ["acme"]

[selector]
height = "30%"
display = "tilde"
"#,
        )
        .unwrap_err()
        .to_string();

        assert!(err.contains("old flat layout"), "{err}");
        assert!(err.contains("[repo]"), "{err}");
        assert!(err.contains(r#"root = "~/dev/github.com""#), "{err}");
        assert!(err.contains(r#"extra = ["~/bin"]"#), "{err}");
        assert!(err.contains(r#"ignore = ["target"]"#), "{err}");
        assert!(err.contains(r#"display = "tilde""#), "{err}");
        assert!(err.contains(r#"labels = { work = ["acme"] }"#), "{err}");
        assert!(err.contains("$EDITOR"), "{err}");
    }

    /// `[picker]` is the name `[selector]` used to have. Reading it anyway would
    /// keep the old spelling alive forever, and ignoring it would silently reset
    /// a customised height — so it is refused, with the keys that were written
    /// handed back under the new heading.
    #[test]
    fn a_picker_table_is_rejected_with_its_new_name() {
        let err = parse_toml(
            r#"
[picker]
height = "30%"
preview = false
preview_window = "down:40%"
"#,
        )
        .unwrap_err()
        .to_string();

        assert!(err.contains("`[picker]`"), "{err}");
        assert!(err.contains("[selector]"), "{err}");
        assert!(err.contains(r#"height = "30%""#), "{err}");
        assert!(err.contains("preview = false"), "{err}");
        assert!(err.contains(r#"preview_window = "down:40%""#), "{err}");
    }

    /// A `[picker]` carrying `display` has two things wrong with it. Reporting
    /// only the table name would send the user back for a second error the
    /// moment they fixed it.
    #[test]
    fn a_picker_table_carrying_display_says_where_display_went() {
        let err = parse_toml("[picker]\ndisplay = \"tilde\"\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains("`[picker]`"), "{err}");
        assert!(err.contains("[repo]"), "{err}");
    }

    /// Advice that does not parse is not advice. Whatever either migration
    /// error prints has to be a config the very next run accepts, or the user
    /// pastes it and gets a second error.
    #[test]
    fn the_generated_replacements_parse() {
        let hint = migration_hint(&LegacyPaths::Grouped(
            [
                (
                    "work".to_string(),
                    vec![entry("~/dev/github.com/capralifecycle", 1)],
                ),
                (
                    "personal".to_string(),
                    vec![entry("~/dev/github.com/joakimen", 1), entry("~/bin", 0)],
                ),
            ]
            .into_iter()
            .collect(),
        ));
        let cfg = parse_toml(&hint).unwrap_or_else(|e| panic!("{e:#}\n\n{hint}"));
        assert_eq!(cfg.repo.label_of("capralifecycle"), Some("work"));
        assert_eq!(cfg.repo.extra, vec!["~/bin".to_string()]);

        let old = r#"
root = "~/dev/github.com"
extra = ["~/bin"]
ignore = ["target"]

[owners]
work = ["acme"]

[selector]
display = "tilde"
"#;
        let raw: RawToml = toml::from_str(old).unwrap();
        let replacement = raw.flat_layout().unwrap().replacement();
        let cfg = parse_toml(&replacement).unwrap_or_else(|e| panic!("{e:#}\n\n{replacement}"));
        assert_eq!(cfg.repo.root.as_deref(), Some("~/dev/github.com"));
        assert_eq!(cfg.repo.extra, vec!["~/bin".to_string()]);
        assert_eq!(cfg.repo.ignore, vec!["target".to_string()]);
        assert_eq!(cfg.repo.display, RepoDisplay::Tilde);
        assert_eq!(cfg.repo.label_of("acme"), Some("work"));

        let renamed = r#"
[picker]
height = "30%"
preview = false
preview_window = "down:40%"
"#;
        let raw: RawToml = toml::from_str(renamed).unwrap();
        let table = renamed_picker_table(raw.picker.as_ref().unwrap());
        let cfg = parse_toml(&table).unwrap_or_else(|e| panic!("{e:#}\n\n{table}"));
        assert_eq!(cfg.selector.height, "30%");
        assert!(!cfg.selector.preview);
        assert_eq!(cfg.selector.preview_window, "down:40%");
    }

    /// Every superseded key is detected on its own — a config that only set
    /// `owners`, or only moved `display`, must not slip through.
    #[test]
    fn each_superseded_key_is_detected_alone() {
        for old in [
            "root = \"~/src\"",
            "extra = [\"~/bin\"]",
            "ignore = [\"target\"]",
            "editor = \"nvim\"",
            "[owners]\nwork = [\"acme\"]",
            "[selector]\ndisplay = \"tilde\"",
        ] {
            let err = parse_toml(old).unwrap_err().to_string();
            assert!(err.contains("old flat layout"), "{old} accepted: {err}");
        }
    }

    /// Only the superseded keys trigger it: a `[selector]` with no `display` is
    /// exactly what the current format asks for.
    #[test]
    fn the_current_layout_is_not_mistaken_for_the_old_one() {
        let cfg = parse_toml(
            r#"
[repo]
root = "~/src"
display = "tilde"

[selector]
height = "30%"
preview = false
"#,
        )
        .unwrap();
        assert_eq!(cfg.repo.display, RepoDisplay::Tilde);
        assert_eq!(cfg.selector.height, "30%");
        assert!(!cfg.selector.preview);
    }

    #[test]
    fn parses_selector_height() {
        let cfg = parse_toml("[selector]\nheight = \"30%\"\n").unwrap();
        assert_eq!(cfg.selector.height, "30%");
    }

    #[test]
    fn parses_repo_display_mode() {
        let cfg = parse_toml("[repo]\ndisplay = \"tilde\"\n").unwrap();
        assert_eq!(cfg.repo.display, RepoDisplay::Tilde);
    }

    #[test]
    fn repo_display_defaults_to_relative() {
        assert_eq!(parse_toml("").unwrap().repo.display, RepoDisplay::Relative);
        assert_eq!(
            parse_toml("[repo]\n").unwrap().repo.display,
            RepoDisplay::Relative
        );
    }

    #[test]
    fn parses_preview_settings() {
        let cfg =
            parse_toml("[selector]\npreview = false\npreview_window = \"down:40%\"\n").unwrap();
        assert!(!cfg.selector.preview);
        assert_eq!(cfg.selector.preview_window, "down:40%");
    }

    #[test]
    fn preview_defaults_to_on_at_the_right() {
        let cfg = parse_toml("").unwrap();
        assert!(cfg.selector.preview);
        assert_eq!(cfg.selector.preview_window, "right:50%");
    }

    #[test]
    fn selector_height_defaults_to_50pct() {
        assert_eq!(parse_toml("").unwrap().selector.height, "50%");
        // A `[selector]` table with no height still gets the default.
        assert_eq!(parse_toml("[selector]\n").unwrap().selector.height, "50%");
    }

    /// Unknown selector keys from older configs (e.g. `backend`) are ignored, not
    /// an error.
    #[test]
    fn selector_ignores_unknown_keys() {
        let cfg = parse_toml("[selector]\nbackend = \"fzf\"\nheight = \"10\"\n").unwrap();
        assert_eq!(cfg.selector.height, "10");
    }

    #[test]
    fn toml_preserves_explicit_empty_ignore() {
        assert!(
            parse_toml("[repo]\nignore = []")
                .unwrap()
                .repo
                .ignore
                .is_empty()
        );
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
    fn editor_precedence_prefers_visual() {
        assert_eq!(
            resolve_editor(Some("code"), Some("vi")).as_deref(),
            Some("code")
        );
        assert_eq!(resolve_editor(None, Some("vi")).as_deref(), Some("vi"));
        assert_eq!(resolve_editor(None, None), None);
    }

    /// `EDITOR=""` means "no editor", not a program named "".
    #[test]
    fn editor_ignores_blank_values() {
        assert_eq!(
            resolve_editor(Some("  "), Some("vi")).as_deref(),
            Some("vi")
        );
        assert_eq!(resolve_editor(Some(""), None), None);
        // A value that is only padded still resolves, trimmed.
        assert_eq!(resolve_editor(Some(" hx "), None).as_deref(), Some("hx"));
    }

    #[test]
    fn splits_editor_into_program_and_args() {
        assert_eq!(split_editor("nvim"), vec!["nvim"]);
        assert_eq!(split_editor("code -w"), vec!["code", "-w"]);
        assert_eq!(split_editor("  nvim   -p  "), vec!["nvim", "-p"]);
    }
}
