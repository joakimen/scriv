//! Command-line entry point: define the argument surface, build [`Ctx`], and
//! dispatch to the [`cmd`] implementations. All decision logic lives in the
//! library crate.
//!
//! Top-level commands: `repo` and `file` work with the things scriv tracks;
//! `config` manages its configuration; `init` prints shell integration. The
//! help layout follows clap's default (description, usage, commands, options).

use std::process::ExitCode;

use clap::builder::styling::{AnsiColor, Effects, Styles};
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::Shell;

use scriv::pick::Cancelled;
use scriv::{Ctx, cmd, shell};

/// Usage examples appended to the top-level help.
const EXAMPLES: &str = "\x1b[1;92mExamples:\x1b[0m
  scriv config init            Write a starter configuration
  scriv repo pick              Fuzzy-pick a repository, print its path
  cd (scriv repo pick)         Jump to a repository (fish)
  scriv file add <path>        Track a file you visit often
  scriv init fish | source     Load shell integration + completions";

/// Help styling matching cargo: bright-green bold headers and usage,
/// bright-cyan bold literals (command and flag names), cyan placeholders.
const STYLES: Styles = Styles::styled()
    .header(AnsiColor::BrightGreen.on_default().effects(Effects::BOLD))
    .usage(AnsiColor::BrightGreen.on_default().effects(Effects::BOLD))
    .literal(AnsiColor::BrightCyan.on_default().effects(Effects::BOLD))
    .placeholder(AnsiColor::Cyan.on_default())
    .error(AnsiColor::Red.on_default().effects(Effects::BOLD))
    .valid(AnsiColor::BrightCyan.on_default().effects(Effects::BOLD))
    .invalid(AnsiColor::Yellow.on_default().effects(Effects::BOLD));

#[derive(Parser)]
#[command(
    name = "scriv",
    version,
    about = "Discover Git repositories and track the files you visit often.",
    after_help = EXAMPLES,
    styles = STYLES,
    disable_help_subcommand = true
)]
struct Cli {
    /// Verbose output
    #[arg(short, long, global = true)]
    verbose: bool,

    /// Path to the config file
    #[arg(short, long, global = true, value_name = "FILE")]
    config: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
#[command(disable_help_subcommand = true)]
enum Command {
    /// Discover and open Git repositories
    Repo {
        #[command(subcommand)]
        command: RepoCmd,
    },
    /// Track the files you visit regularly
    File {
        #[command(subcommand)]
        command: FileCmd,
    },
    /// Manage the configuration
    Config {
        #[command(subcommand)]
        command: ConfigCmd,
    },
    /// Print shell integration for `source`-ing
    ///
    /// `fish` emits helper functions, key bindings, and completions; other
    /// shells emit completions only.
    Init {
        /// Shell to emit integration for
        shell: Shell,
    },
}

#[derive(Subcommand)]
enum RepoCmd {
    /// List all discovered repositories
    #[command(alias = "list")]
    Ls {
        /// Return absolute file paths
        #[arg(short = 'A', long)]
        absolute_paths: bool,
    },
    /// Fuzzy-select a repository and print its absolute path
    Pick,
}

#[derive(Subcommand)]
enum FileCmd {
    /// List known files
    #[command(alias = "list")]
    Ls {
        /// Show file existence indicators
        #[arg(long)]
        status: bool,
        /// Only show files that do not exist locally
        #[arg(long, conflicts_with = "exists")]
        missing: bool,
        /// Only show files that exist locally
        #[arg(long)]
        exists: bool,
    },
    /// Fuzzy-select a known file and print its absolute path
    Pick,
    /// Add a file; omit the path to pick one from the current directory
    Add {
        /// File path to add
        file: Option<String>,
    },
    /// Remove a file; omit the path to choose interactively
    #[command(alias = "forget")]
    Remove {
        /// File path to remove
        file: Option<String>,
    },
}

#[derive(Subcommand)]
enum ConfigCmd {
    /// Generate a starter configuration file
    Init {
        /// Overwrite an existing configuration file
        #[arg(short, long)]
        force: bool,
    },
    /// Print the resolved configuration
    Print,
    /// Print the configuration file path
    Path,
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    // `init` needs the clap command, not the environment; handle it before Ctx.
    if let Command::Init { shell } = &cli.command {
        print!("{}", shell::integration(*shell, &mut Cli::command()));
        return ExitCode::SUCCESS;
    }

    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        // A cancelled picker is a silent, conventional exit, not an error.
        Err(err) if err.is::<Cancelled>() => ExitCode::from(130),
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> anyhow::Result<()> {
    let ctx = Ctx::load(cli.config.as_deref(), cli.verbose)?;

    match cli.command {
        Command::Repo { command } => match command {
            RepoCmd::Ls { absolute_paths } => cmd::repo::ls(&ctx, absolute_paths),
            RepoCmd::Pick => cmd::repo::pick(&ctx),
        },
        Command::File { command } => match command {
            FileCmd::Ls {
                status,
                missing,
                exists,
            } => cmd::file::ls(&ctx, status, missing, exists),
            FileCmd::Pick => cmd::file::pick(&ctx),
            FileCmd::Add { file } => cmd::file::add(&ctx, file.as_deref()),
            FileCmd::Remove { file } => cmd::file::remove(&ctx, file.as_deref()),
        },
        Command::Config { command } => match command {
            ConfigCmd::Init { force } => cmd::config::init(&ctx, force),
            ConfigCmd::Print => cmd::config::print(&ctx),
            ConfigCmd::Path => cmd::config::path(&ctx),
        },
        Command::Init { .. } => unreachable!("init handled before Ctx"),
    }
}
