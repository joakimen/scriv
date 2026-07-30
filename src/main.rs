//! Command-line entry point: define the argument surface, build [`Ctx`], and
//! dispatch to the [`cmd`] implementations. All decision logic lives in the
//! library crate.
//!
//! Top-level commands: `repo`, `file`, `branch`, and `pr` work with the things
//! scriv finds; `edit` opens a file from the directory you are in; `config`
//! manages its configuration; `init` prints shell integration. The help layout
//! follows clap's default (description, usage, commands, options).

use std::process::ExitCode;

use clap::builder::styling::{AnsiColor, Effects, Styles};
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::Shell;

use scriv::gh::MergeMethod;
use scriv::git::Filter;
use scriv::pick::Cancelled;
use scriv::{Ctx, Reported, cmd, shell};

/// Usage examples appended to the top-level help.
const EXAMPLES: &str = "\x1b[1;92mExamples:\x1b[0m
  scriv config init            Write a starter configuration
  scriv repo pick              Fuzzy-pick a repository, print its path
  cd (scriv repo pick)         Jump to a repository (fish)
  scriv repo clone             Pick an owner, then repositories to clone
  scriv file add <path>        Track a file you visit often
  scriv edit                   Pick a file under the current directory, edit it
  scriv edit --tracked         Pick one of your tracked files and edit it
  scriv branch checkout        Pick a local or remote branch and switch to it
  scriv pr checkout            Pick a GitHub pull request and check it out
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
    about = "Pick your repositories, files, git branches and GitHub pull requests from one fuzzy finder.",
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
    /// List and pick the Git repositories under your search paths
    Repo {
        #[command(subcommand)]
        command: RepoCmd,
    },
    /// Track the files you visit regularly
    File {
        #[command(subcommand)]
        command: FileCmd,
    },
    /// Fuzzy-find a file and open it in your editor
    ///
    /// Selection comes from the current directory tree, honouring `.gitignore`,
    /// or from your tracked files with `--tracked`. Selecting several opens them
    /// all. The editor is `$VISUAL`, then `$EDITOR`.
    #[command(alias = "e")]
    Edit {
        /// Files to open; omit to pick interactively
        #[arg(value_name = "FILE")]
        files: Vec<String>,
        /// Pick from your tracked files instead of the current directory
        #[arg(short, long, conflicts_with = "files")]
        tracked: bool,
    },
    /// Switch between local and remote git branches
    ///
    /// Listings lead with the current branch, then local branches, then
    /// remote-only ones, each most recently committed to first. In a branch
    /// picker, ctrl-r fetches from every remote and reloads the list without
    /// closing the picker.
    #[command(alias = "br")]
    Branch {
        #[command(subcommand)]
        command: BranchCmd,
    },
    /// Work with GitHub pull requests (via the `gh` CLI)
    ///
    /// In a pull request picker, ctrl-r asks GitHub again and reloads the list
    /// in place, for when a check has finished while you were looking at it.
    Pr {
        #[command(subcommand)]
        command: PrCmd,
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
    /// List every repository found under your search paths
    #[command(alias = "list")]
    Ls {
        /// Return absolute file paths
        #[arg(short = 'A', long)]
        absolute_paths: bool,
    },
    /// Fuzzy-select a repository and print its absolute path
    Pick,
    /// Clone repositories from GitHub into your root
    ///
    /// With no argument, pick an owner — suggested from your config, the owners
    /// already under your root, and your own GitHub account — or type any other.
    /// Then fuzzy-select one or more of that owner's repositories; `tab` selects
    /// several and they clone concurrently. `owner/repo` skips both pickers.
    ///
    /// Everything lands at `<root>/<owner>/<repo>`, so a clone is in
    /// `scriv repo pick` immediately afterwards. Repositories you already have
    /// are listed but greyed, and are skipped rather than re-cloned.
    Clone {
        /// `owner` to pick from, or `owner/repo` to clone directly
        #[arg(value_name = "OWNER[/REPO]")]
        target: Option<String>,
        /// Maximum number of repositories to fetch for an owner
        #[arg(short = 'L', long, default_value_t = 1000)]
        limit: usize,
    },
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

/// Which branches a `branch` subcommand considers, shared by all three.
#[derive(clap::Args)]
struct BranchScope {
    /// Only branches that exist in this clone
    #[arg(short = 'l', long, conflicts_with = "remote")]
    local: bool,
    /// Only branches that exist on a remote
    #[arg(short = 'r', long)]
    remote: bool,
    /// Fetch from all remotes (pruning deleted branches) first
    #[arg(short = 'f', long)]
    fetch: bool,
}

impl BranchScope {
    fn filter(&self) -> Filter {
        Filter::from_flags(self.local, self.remote)
    }
}

#[derive(Subcommand)]
enum BranchCmd {
    /// List local and remote branches
    #[command(alias = "list")]
    Ls {
        /// Show the current-branch marker, local/both/remote tag, and last commit
        #[arg(long)]
        status: bool,
        #[command(flatten)]
        scope: BranchScope,
    },
    /// Fuzzy-select a branch and print its name
    Pick {
        #[command(flatten)]
        scope: BranchScope,
    },
    /// Check out a branch; omit the name to pick one
    ///
    /// Checking out a remote-only branch creates the matching local branch and
    /// sets its upstream.
    #[command(aliases = ["co", "switch"])]
    Checkout {
        /// Branch to check out, e.g. `main` or `origin/feature`
        branch: Option<String>,
        #[command(flatten)]
        scope: BranchScope,
    },
}

/// Which pull requests a `pr` subcommand fetches, shared by all three.
#[derive(clap::Args)]
struct PrScope {
    /// Pull request state to include
    #[arg(short, long, default_value = "open", value_parser = ["open", "closed", "merged", "all"])]
    state: String,
    /// Maximum number of pull requests to fetch
    #[arg(short = 'L', long, default_value_t = 50)]
    limit: usize,
}

/// How to merge, when the user has decided. With none of these, `gh` asks.
///
/// No short flags: `-s` is already `--state` on every `pr` subcommand, and
/// offering `-m`/`-r` but not `-s` would be worse than offering none.
#[derive(clap::Args)]
struct MergeMethodArg {
    /// Merge with a merge commit
    #[arg(long, conflicts_with_all = ["squash", "rebase"])]
    merge: bool,
    /// Squash the commits into one
    #[arg(long, conflicts_with = "rebase")]
    squash: bool,
    /// Rebase the commits onto the base branch
    #[arg(long)]
    rebase: bool,
}

impl MergeMethodArg {
    fn method(&self) -> Option<MergeMethod> {
        match (self.merge, self.squash, self.rebase) {
            (true, _, _) => Some(MergeMethod::Merge),
            (_, true, _) => Some(MergeMethod::Squash),
            (_, _, true) => Some(MergeMethod::Rebase),
            _ => None,
        }
    }
}

#[derive(Subcommand)]
enum PrCmd {
    /// List pull requests
    #[command(alias = "list")]
    Ls {
        /// Also show the state tag, source branch, and last-updated date
        ///
        /// The check and conflict marks are in the plain listing already.
        #[arg(long)]
        status: bool,
        #[command(flatten)]
        scope: PrScope,
    },
    /// Fuzzy-select a pull request and print its number
    Pick {
        #[command(flatten)]
        scope: PrScope,
    },
    /// Check out a pull request's branch; omit the number to pick one
    #[command(alias = "co")]
    Checkout {
        /// Pull request number
        number: Option<u64>,
        #[command(flatten)]
        scope: PrScope,
    },
    /// Open a pull request in the browser; omit the number to pick one
    Open {
        /// Pull request number
        number: Option<u64>,
        #[command(flatten)]
        scope: PrScope,
    },
    /// Merge a pull request; omit the number to pick one
    ///
    /// The picker colours each row by whether it can actually be merged — green
    /// ready, yellow waiting on checks, red blocked, grey draft or closed — so
    /// you see what you are merging as you choose it. With no merge method
    /// given, `gh` asks for one.
    Merge {
        /// Pull request number
        number: Option<u64>,
        #[command(flatten)]
        method: MergeMethodArg,
        /// Delete the source branch after merging
        #[arg(short, long)]
        delete_branch: bool,
        /// Merge once the required checks pass, rather than now
        #[arg(long)]
        auto: bool,
        #[command(flatten)]
        scope: PrScope,
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
        // git and gh explain their own failures; pass their status through
        // rather than printing a second, vaguer line on top.
        Err(err) if err.is::<Reported>() => match err.downcast_ref::<Reported>() {
            Some(Reported(code)) if *code > 0 => ExitCode::from(*code as u8),
            _ => ExitCode::FAILURE,
        },
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
            RepoCmd::Clone { target, limit } => cmd::repo::clone(&ctx, target.as_deref(), limit),
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
        Command::Edit { files, tracked } => cmd::edit::run(&ctx, &files, tracked),
        Command::Branch { command } => match command {
            BranchCmd::Ls { status, scope } => {
                cmd::branch::ls(&ctx, scope.filter(), status, scope.fetch)
            }
            BranchCmd::Pick { scope } => cmd::branch::pick(&ctx, scope.filter(), scope.fetch),
            BranchCmd::Checkout { branch, scope } => {
                cmd::branch::checkout(&ctx, branch.as_deref(), scope.filter(), scope.fetch)
            }
        },
        Command::Pr { command } => match command {
            PrCmd::Ls { status, scope } => cmd::pr::ls(&ctx, &scope.state, scope.limit, status),
            PrCmd::Pick { scope } => cmd::pr::pick(&ctx, &scope.state, scope.limit),
            PrCmd::Checkout { number, scope } => {
                cmd::pr::checkout(&ctx, number, &scope.state, scope.limit)
            }
            PrCmd::Open { number, scope } => cmd::pr::open(&ctx, number, &scope.state, scope.limit),
            PrCmd::Merge {
                number,
                method,
                delete_branch,
                auto,
                scope,
            } => cmd::pr::merge(
                &ctx,
                number,
                &scope.state,
                scope.limit,
                method.method(),
                delete_branch,
                auto,
            ),
        },
        Command::Config { command } => match command {
            ConfigCmd::Init { force } => cmd::config::init(&ctx, force),
            ConfigCmd::Print => cmd::config::print(&ctx),
            ConfigCmd::Path => cmd::config::path(&ctx),
        },
        Command::Init { .. } => unreachable!("init handled before Ctx"),
    }
}
