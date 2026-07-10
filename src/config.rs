//! Configuration model and loading. Parsing and path resolution are pure —
//! they take environment and home values as arguments — so the only I/O is the
//! single file read in [`load_config`].

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

use crate::path::expand_home_dir;

/// Overrides the resolved config file path when set.
pub const CONFIG_ENV_VAR: &str = "SCRIV_CONFIG";
/// Base directory for the default config location, per the XDG spec.
pub const XDG_ENV_VAR: &str = "XDG_CONFIG_HOME";

const DEFAULT_IGNORED_DIRS: &[&str] = &["node_modules", "vendor", "dist", "build", "target"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub paths: Vec<PathEntry>,
    pub ignore: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PathEntry {
    pub path: String,
    #[serde(default)]
    pub depth: usize,
}

#[derive(Deserialize)]
struct RawConfig {
    #[serde(default)]
    paths: Vec<PathEntry>,
    ignore: Option<Vec<String>>,
}

/// Parse config from JSON. A missing `ignore` key falls back to the default
/// ignore list; an explicit empty list is preserved.
pub fn parse_config(data: &str) -> Result<Config> {
    let raw: RawConfig = serde_json::from_str(data).context("parsing configuration file")?;
    Ok(Config {
        paths: raw.paths,
        ignore: raw.ignore.unwrap_or_else(default_ignored_dirs),
    })
}

/// Read and parse the config file at `path`.
pub fn load_config(path: &Path) -> Result<Config> {
    let data = std::fs::read_to_string(path)
        .with_context(|| format!("reading configuration file: {}", path.display()))?;
    parse_config(&data)
}

/// Resolve the config file path by precedence:
/// explicit `flag` > `SCRIV_CONFIG` > `XDG_CONFIG_HOME` > `~/.config`.
pub fn resolve_config_path(
    flag: Option<&str>,
    scriv_env: Option<&str>,
    xdg_env: Option<&str>,
    home: &Path,
) -> PathBuf {
    if let Some(flag) = flag.filter(|s| !s.is_empty()) {
        return expand_home_dir(flag, home);
    }
    if let Some(env) = scriv_env.filter(|s| !s.is_empty()) {
        return expand_home_dir(env, home);
    }
    let base = match xdg_env.filter(|s| !s.is_empty()) {
        Some(xdg) => PathBuf::from(xdg),
        None => home.join(".config"),
    };
    base.join("scriv").join("config.json")
}

fn default_ignored_dirs() -> Vec<String> {
    DEFAULT_IGNORED_DIRS.iter().map(|s| s.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_config() {
        let cfg = parse_config(
            r#"{ "paths": [{"path": "~/dev", "depth": 2}], "ignore": ["foo", "bar"] }"#,
        )
        .unwrap();
        assert_eq!(
            cfg,
            Config {
                paths: vec![PathEntry {
                    path: "~/dev".into(),
                    depth: 2
                }],
                ignore: vec!["foo".into(), "bar".into()],
            }
        );
    }

    #[test]
    fn applies_default_ignore_when_absent() {
        let cfg = parse_config(r#"{ "paths": [{"path": "~/dev"}] }"#).unwrap();
        assert_eq!(cfg.ignore, default_ignored_dirs());
        assert_eq!(cfg.paths[0].depth, 0);
    }

    #[test]
    fn preserves_explicit_empty_ignore() {
        let cfg = parse_config(r#"{ "paths": [], "ignore": [] }"#).unwrap();
        assert!(cfg.ignore.is_empty());
    }

    #[test]
    fn rejects_invalid_json() {
        assert!(parse_config("{not json").is_err());
    }

    #[test]
    fn flag_takes_precedence() {
        let home = Path::new("/home/user");
        let got = resolve_config_path(Some("~/custom.json"), Some("/env.json"), Some("/xdg"), home);
        assert_eq!(got, home.join("custom.json"));
    }

    #[test]
    fn env_beats_xdg_and_home() {
        let home = Path::new("/home/user");
        let got = resolve_config_path(None, Some("/env.json"), Some("/xdg"), home);
        assert_eq!(got, PathBuf::from("/env.json"));
    }

    #[test]
    fn xdg_beats_home() {
        let home = Path::new("/home/user");
        let got = resolve_config_path(None, None, Some("/xdg"), home);
        assert_eq!(got, PathBuf::from("/xdg/scriv/config.json"));
    }

    #[test]
    fn falls_back_to_home_config() {
        let home = Path::new("/home/user");
        let got = resolve_config_path(None, None, None, home);
        assert_eq!(got, home.join(".config/scriv/config.json"));
    }

    #[test]
    fn ignores_empty_env_values() {
        let home = Path::new("/home/user");
        let got = resolve_config_path(Some(""), Some(""), Some(""), home);
        assert_eq!(got, home.join(".config/scriv/config.json"));
    }
}
