//! `scriv` — select repositories, files, git branches, GitHub pull requests and
//! running processes from one fuzzy finder.
//!
//! The crate is split into an I/O-free core ([`config`], [`path`], [`files`]'s
//! pure helpers, [`repo`]'s traversal rules, [`history`]'s fish-history
//! parsing, [`proc`]'s `ps` parsing, [`git`] and [`gh`]'s parsing and
//! classification) and an imperative shell: the [`cmd`] modules that read the
//! environment, touch the filesystem, shell out to `git`/`gh`, and drive
//! interactive selection. [`Ctx`] resolves the environment once and hands it to
//! every command.

pub mod cmd;
pub mod config;
pub mod files;
pub mod gh;
pub mod git;
pub mod history;
pub mod logger;
pub mod path;
pub mod proc;
pub mod repo;
pub mod select;
pub mod shell;
pub mod term;
pub mod walk;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use config::Config;
use logger::Logger;

/// A subprocess that already explained its own failure on stderr — `git` and
/// `gh` both do, in their own well-known wording.
///
/// Propagated instead of a message so the command exits with the child's status
/// without scriv restating what the user just read.
#[derive(Debug)]
pub struct Reported(pub i32);

impl std::fmt::Display for Reported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "subprocess exited with status {}", self.0)
    }
}

impl std::error::Error for Reported {}

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
    /// fish's history file, which `scriv history` reads.
    pub history_path: PathBuf,
    /// This machine's offset from UTC, for dating history entries. See
    /// [`Ctx::load`] for why it is read here and not where it is used.
    utc_offset: time::UtcOffset,
    /// The editor `scriv edit` launches, from the environment.
    editor: Option<String>,
    /// Whether printed output carries colour, resolved once from `--color`,
    /// `SCRIV_NO_COLOR` and whether stdout is a terminal.
    color: bool,
    pub config: Config,
    pub log: Logger,
}

impl Ctx {
    /// Resolve the environment and load configuration.
    pub fn load(
        config_flag: Option<&str>,
        verbose: bool,
        color: term::ColorChoice,
    ) -> Result<Self> {
        let home = dirs::home_dir().context("determining home directory")?;
        let cwd = std::env::current_dir().context("determining working directory")?;
        // `$PWD` keeps the symlinks the user walked through, which `getcwd`
        // resolves away — but only when it is still current. See
        // [`path::resolve_pwd`].
        let pwd = path::resolve_pwd(
            std::env::var("PWD").ok().as_deref(),
            cwd,
            |claimed, actual| match (claimed.canonicalize(), actual.canonicalize()) {
                (Ok(a), Ok(b)) => a == b,
                _ => false,
            },
        );

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

        // fish's data directory, not its config directory — a different XDG
        // variable from the one above.
        let history_path = history::history_path(
            config.history.file.as_deref(),
            std::env::var(history::XDG_DATA_ENV_VAR).ok().as_deref(),
            &home,
        );

        // Read here, at the top of the process, because it cannot be read
        // later: determining the local offset reads the environment, which is
        // unsound once another thread might be changing it, so `time` refuses
        // to answer at all in a multi-threaded process on Unix. By the time a
        // history row wants dating, skim has threads running. Resolving it once
        // and carrying it is also simply what `Ctx` is for.
        //
        // UTC is the fallback if it cannot be determined — a listing an hour
        // out beats a listing that refuses to open.
        let utc_offset = time::UtcOffset::current_local_offset().unwrap_or(time::UtcOffset::UTC);

        let editor = config::resolve_editor(
            std::env::var("VISUAL").ok().as_deref(),
            std::env::var("EDITOR").ok().as_deref(),
        );

        Ok(Self {
            home_s: home.to_string_lossy().into_owned(),
            pwd_s: pwd.to_string_lossy().into_owned(),
            home,
            config_path,
            files_path,
            legacy_kf_path,
            history_path,
            utc_offset,
            editor,
            // Resolved here rather than at each listing, so one run cannot
            // colour one command's output and not another's.
            color: color.for_stdout(),
            config,
            log: Logger::new(verbose),
        })
    }

    /// Whether printed output should carry ANSI colour.
    ///
    /// The selector is not governed by this: it is a full terminal UI that only
    /// ever draws on a terminal, and `--color` is about what scriv *prints* —
    /// the same scope ripgrep and fd give it.
    pub fn color(&self) -> bool {
        self.color
    }

    /// The resolved editor command, or `None` when nothing is set. For
    /// reporting; [`Ctx::editor`] is what launching goes through.
    pub fn editor_setting(&self) -> Option<&str> {
        self.editor.as_deref()
    }

    /// The editor command to launch, split into program and arguments.
    ///
    /// Errors when nothing is set, naming both variables the user can set
    /// rather than failing on an empty program name deeper in.
    pub fn editor(&self) -> Result<Vec<String>> {
        let command = self
            .editor
            .as_deref()
            .context("no editor set — set $VISUAL or $EDITOR")?;
        let parts = config::split_editor(command);
        if parts.is_empty() {
            anyhow::bail!("the configured editor is empty");
        }
        Ok(parts)
    }

    /// This machine's offset from UTC, resolved once at startup.
    pub fn utc_offset(&self) -> time::UtcOffset {
        self.utc_offset
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
