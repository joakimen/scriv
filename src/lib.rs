//! `scriv` — discover Git repositories and track the files you visit often.
//!
//! The crate is split into an I/O-free core ([`config`], [`path`], [`files`]'s
//! pure helpers, [`repo`]'s traversal rules) and an imperative shell: the
//! [`cmd`] modules that read the environment, touch the filesystem, and drive
//! interactive selection. [`Ctx`] resolves the environment once and hands it to
//! every command.

pub mod cmd;
pub mod config;
pub mod files;
pub mod logger;
pub mod path;
pub mod pick;
pub mod repo;
pub mod shell;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use config::Config;
use logger::Logger;

/// Resolved runtime environment shared by every command.
///
/// Built once from the process environment and CLI flags, then passed by
/// reference so the command implementations stay free of environment lookups.
pub struct Ctx {
    home: PathBuf,
    home_s: String,
    pwd_s: String,
    /// The resolved config file (`config.toml`, or a legacy `config.json`).
    pub config_path: PathBuf,
    /// The known-files list, beside the config file.
    pub files_path: PathBuf,
    /// The standalone `kf` tool's config, read once to migrate its list.
    pub legacy_kf_path: PathBuf,
    pub config: Config,
    pub log: Logger,
}

impl Ctx {
    /// Resolve the environment and load configuration.
    pub fn load(config_flag: Option<&str>, verbose: bool) -> Result<Self> {
        let home = dirs::home_dir().context("determining home directory")?;
        let pwd = std::env::var("PWD")
            .map(PathBuf::from)
            .or_else(|_| std::env::current_dir())
            .context("determining working directory")?;

        let scriv_env = std::env::var(config::CONFIG_ENV_VAR).ok();
        let xdg_env = std::env::var(config::XDG_ENV_VAR).ok();

        let config_path = config::resolve_config_path(
            config_flag,
            scriv_env.as_deref(),
            xdg_env.as_deref(),
            &home,
            |p| p.exists(),
        );
        let files_path = config::files_path(&config_path);
        let legacy_kf_path = config::legacy_kf_path(xdg_env.as_deref(), &home);
        let config = config::load_config(&config_path)?;

        Ok(Self {
            home_s: home.to_string_lossy().into_owned(),
            pwd_s: pwd.to_string_lossy().into_owned(),
            home,
            config_path,
            files_path,
            legacy_kf_path,
            config,
            log: Logger::new(verbose),
        })
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    pub fn home_str(&self) -> &str {
        &self.home_s
    }

    pub fn pwd_str(&self) -> &str {
        &self.pwd_s
    }

    /// Copy the standalone `kf` tool's list into place on first use.
    ///
    /// Runs only when the known-files list does not yet exist and a legacy `kf`
    /// config does; idempotent thereafter. The number migrated is reported on
    /// stderr so the one-time move is visible without polluting stdout.
    pub fn ensure_files_migrated(&self) -> Result<()> {
        if self.files_path.exists() || !self.legacy_kf_path.exists() {
            return Ok(());
        }
        let lines = files::read_lines(&self.legacy_kf_path)?;
        let normalized = files::normalize_entries(&lines);
        if normalized.is_empty() {
            return Ok(());
        }
        files::write_lines(&self.files_path, &normalized)?;
        eprintln!(
            "migrated {} entries from {}",
            normalized.len(),
            self.legacy_kf_path.display()
        );
        Ok(())
    }
}
