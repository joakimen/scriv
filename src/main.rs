//! Command-line entry point and imperative shell: parse arguments, read the
//! config file and environment, run discovery, and print results. All decision
//! logic lives in the [`config`], [`discover`], and [`path`] modules, which are
//! free of process state.

mod config;
mod discover;
mod logger;
mod path;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use config::{CONFIG_ENV_VAR, Config, XDG_ENV_VAR, load_config, resolve_config_path};
use logger::Logger;
use path::format_repo_path;

#[derive(Parser)]
#[command(
    name = "scriv",
    version,
    about = "scriv is a tool for discovering Git repositories."
)]
struct Cli {
    /// Verbose output
    #[arg(short, long, global = true)]
    verbose: bool,

    /// Path to the config file
    #[arg(short, long, global = true, value_name = "FILE")]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List all repositories discovered using the paths in the user configuration
    #[command(name = "ls", alias = "list")]
    Ls {
        /// Return absolute file paths
        #[arg(short = 'A', long)]
        absolute_paths: bool,
    },
    /// Print current configuration
    Config,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let log = Logger::new(cli.verbose);
    let home = dirs::home_dir().context("determining home directory")?;

    let flag = cli
        .config
        .as_deref()
        .map(|p| p.to_string_lossy().into_owned());
    let scriv_env = std::env::var(CONFIG_ENV_VAR).ok();
    let xdg_env = std::env::var(XDG_ENV_VAR).ok();
    let cfg_path = resolve_config_path(
        flag.as_deref(),
        scriv_env.as_deref(),
        xdg_env.as_deref(),
        &home,
    );

    let cfg = load_config(&cfg_path)?;

    match cli.command {
        Command::Ls { absolute_paths } => run_list(&cfg, &home, absolute_paths, &log),
        Command::Config => run_config(&cfg, &cfg_path, &log),
    }
}

fn run_list(cfg: &Config, home: &Path, absolute: bool, log: &Logger) -> Result<()> {
    log.info(&format!("settings: ignore = {}", cfg.ignore.join(", ")));

    let mut repos = discover::find_all_repos(cfg, home, log)?;
    repos.sort_by(|a, b| a.as_os_str().cmp(b.as_os_str()));

    if repos.is_empty() {
        anyhow::bail!("no repositories found");
    }

    log.info(&format!("returning {} repositories", repos.len()));
    let home = home.to_string_lossy();
    for repo in &repos {
        println!(
            "{}",
            format_repo_path(&repo.to_string_lossy(), &home, absolute)
        );
    }
    Ok(())
}

fn run_config(cfg: &Config, cfg_path: &Path, log: &Logger) -> Result<()> {
    log.info(&format!(
        "printing configuration from {}",
        cfg_path.display()
    ));

    println!("paths:");
    for entry in &cfg.paths {
        println!("  - {} (depth: {})", entry.path, entry.depth);
    }
    println!();
    println!("ignore: {}", cfg.ignore.join(", "));
    Ok(())
}
