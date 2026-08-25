//! Command-line entry point: define the argument surface, build [`Ctx`], and
//! dispatch to the [`cmd`] implementations. All decision logic lives in the
//! library crate.
//!
//! Top-level commands: `repo`, `file`, `note`, `branch`, `worktree`, `pr`,
//! `proc` and `history` work with the things scriv finds; `edit` opens a file
//! from the directory the user is in; `config` manages its configuration;
//! `init` prints shell integration.

use std::process::ExitCode;

use clap::builder::styling::{AnsiColor, Effects, Styles};
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::Shell;

use scriv::gh::MergeMethod;
use scriv::git::Filter;
use scriv::select::Cancelled;
use scriv::term::ColorChoice;
use scriv::{Ctx, Reported, cmd, shell};

/// Usage examples appended to the top-level help. Three lines, and it stays
/// three — see CLAUDE.md.
const EXAMPLES: &str = "\x1b[1;92mExamples:\x1b[0m
  scriv pr checkout            Select a GitHub pull request and check it out
  scriv branch switch          Select a branch and switch to it
  scriv history sel            Search the commands you have already run";

/// What `--help` opens with. The two keys every selector answers to live here
/// rather than in each selector's header: a header is one line competing with
/// the preview pane for the width, and it earns its place by naming what *this*
/// selector can do and another cannot.
const LONG_ABOUT: &str = "Provides fuzzy-completion for various local and remote resources.

Every selector hides and shows its preview pane on ctrl-v, and takes several \
rows on tab wherever several are allowed. Whatever else one can do — open a \
pull request in the browser, refresh the list from GitHub — is named in its \
own header.";

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
    version = scriv::VERSION,
    about = "Provides fuzzy-completion for various local and remote resources.",
    long_about = LONG_ABOUT,
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

    /// When to colour printed output
    ///
    /// `auto` colours a terminal and honours `SCRIV_NO_COLOR`. `always` colours
    /// a pipe or a file too, for a pager such as `less -R`. Either explicit
    /// value overrides `SCRIV_NO_COLOR`. The selector is unaffected — it only
    /// ever draws on a terminal.
    ///
    /// The variable is scriv's own; the cross-tool `NO_COLOR` is not read.
    #[arg(long, global = true, value_name = "WHEN", default_value = "auto")]
    color: ColorChoice,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
#[command(disable_help_subcommand = true)]
enum Command {
    /// Manage local Git repositories
    #[command(visible_alias = "r")]
    Repo {
        #[command(subcommand)]
        command: RepoCmd,
    },
    /// Manage commonly used files
    #[command(visible_alias = "f")]
    File {
        #[command(subcommand)]
        command: FileCmd,
    },
    /// Open files and directories in $EDITOR
    ///
    /// Selection comes from the current directory tree, honouring `.gitignore`.
    /// Selecting several opens them all. The editor is `$VISUAL`, then
    /// `$EDITOR`.
    ///
    /// With no subcommand this is `edit file`, which is what it is nearly
    /// always reached for. A file or directory actually named `file` or `dir`
    /// needs `./file` to tell it apart from the subcommand.
    #[command(visible_alias = "e", args_conflicts_with_subcommands = true)]
    Edit {
        #[command(subcommand)]
        command: Option<EditCmd>,
        /// Files to open; omit to select interactively
        #[arg(value_name = "FILE")]
        files: Vec<String>,
        /// Select from your tracked files instead of the current directory
        #[arg(short, long, conflicts_with = "files")]
        tracked: bool,
    },
    /// Manage the notes in your vault
    ///
    /// The set is every Markdown file under `[note] root` — an Obsidian vault,
    /// or any tree of notes — most recently modified first.
    ///
    /// A row leads with what the note calls itself, then says what is true of
    /// it in a column each: the label its directory carries — or the directory
    /// itself, where it carries none — the folder below that, and its tags.
    /// Behind those, and never searched, come how many of its tasks are still
    /// open and how long ago it was modified and created.
    ///
    /// Titles, tags and creation dates come from a note's YAML front matter and
    /// from nowhere else — inline `#tags` in the body are not indexed.
    ///
    /// `[note] labels` names the directories directly below the root, one label
    /// to many directories, the way `[repo] labels` names owners.
    #[command(visible_alias = "n")]
    Note {
        #[command(subcommand)]
        command: NoteCmd,
    },
    /// Manage local and remote Git branches
    ///
    /// Listings lead with the current branch, then local branches, then
    /// remote-only ones, each most recently committed to first. In a branch
    /// selector, ctrl-r fetches from every remote and reloads the list without
    /// closing the selector.
    #[command(visible_alias = "b")]
    Branch {
        #[command(subcommand)]
        command: BranchCmd,
    },
    /// Manage this repository's worktrees
    ///
    /// Lists the working trees of the repository you are standing in — the one
    /// cloned and every one added with `git worktree add` — in git's own order,
    /// the main tree first. The tree you are in is marked rather than moved to
    /// the top, since it is not the one you are switching to.
    ///
    /// Switching to a worktree is a `cd`, which only a shell can perform, so
    /// `sel` prints the path; the fish integration binds that to ctrl-t.
    #[command(visible_alias = "w")]
    Worktree {
        #[command(subcommand)]
        command: WorktreeCmd,
    },
    /// Manage GitHub PRs
    ///
    /// In a pull request selector, ctrl-r asks GitHub again and reloads the list
    /// in place, for when a check has finished while you were looking at it.
    ///
    /// f2 opens the highlighted pull request in the browser and f7 checks it
    /// out, whichever verb the selector was opened for — the same keys that do
    /// those things from the prompt in fish. Answering "which one" is the work;
    /// which verb you meant is a key, not another command.
    Pr {
        #[command(subcommand)]
        command: PrCmd,
    },
    /// Manage system processes
    ///
    /// Rows come from a single `ps` call, busiest first, and carry the whole
    /// command line — arguments included — so a process is recognisable by what
    /// it was started with rather than by its name alone. scriv's own process
    /// and everything that spawned it are never listed: killing the shell or
    /// the terminal is not a thing to be one keystroke away from.
    ///
    /// `--port` narrows all three verbs to what is listening on a TCP port,
    /// which is `lsof`'s answer rather than `ps`'s.
    #[command(visible_alias = "pc")]
    Proc {
        #[command(subcommand)]
        command: ProcCmd,
    },
    /// Manage shell history
    ///
    /// Reads fish's history file directly — newest first, with repeats of a
    /// command collapsed onto the one row and dated with when it was last run.
    /// `history sel` prints the command rather than running it; the fish
    /// integration puts it back on the command line, bound to ctrl-r and to
    /// `up` on the first line of a prompt.
    ///
    /// The date is shown but never searched: it is digits at the front of every
    /// row, and matching it would rank timestamps above commands.
    ///
    /// scriv's own `scriv-` shell functions are left out — pressing ctrl-o
    /// records one, and a key press is not a command anyone typed.
    #[command(visible_alias = "h")]
    History {
        #[command(subcommand)]
        command: HistoryCmd,
    },
    /// Manage the configuration
    #[command(visible_alias = "c")]
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
enum EditCmd {
    /// Fuzzy-find a file and open it
    ///
    /// What `scriv edit` does with no subcommand.
    #[command(visible_alias = "f")]
    File {
        /// Files to open; omit to select interactively
        #[arg(value_name = "FILE")]
        files: Vec<String>,
        /// Select from your tracked files instead of the current directory
        #[arg(short, long, conflicts_with = "files")]
        tracked: bool,
    },
    /// Fuzzy-find a directory and open it
    ///
    /// The preview pane is what is directly inside each one. What an editor
    /// does with a directory is its own business — a file browser, a project
    /// root, a tree pane; scriv's part is finding it without a `cd` and a `ls`
    /// per level.
    #[command(visible_alias = "d")]
    Dir {
        /// Directories to open; omit to select interactively
        #[arg(value_name = "DIR")]
        dirs: Vec<String>,
    },
}

#[derive(Subcommand)]
enum RepoCmd {
    /// List every repository found under your search paths
    #[command(visible_alias = "list")]
    Ls {
        /// Return absolute file paths
        #[arg(short = 'A', long)]
        absolute_paths: bool,
    },
    /// Fuzzy-select a repository and print its absolute path
    ///
    /// The ones you have chosen before come first, most-used-recently first;
    /// the rest stay in path order below them. `[selector] recent = false`
    /// turns that off, and stops the choices being recorded. `ls` is unaffected
    /// — the set it prints keeps one order so it can be piped.
    ///
    /// f1 opens the highlighted repository on GitHub instead of printing it,
    /// as it does from the prompt in fish. Nothing is printed then, which the
    /// `cd` wrapper already treats as nothing to do.
    Sel,
    /// Open a repository's GitHub page in the browser
    ///
    /// Inside a repository, opens that one. Anywhere else, fuzzy-selects from
    /// every repository under your search paths. Either way it hands off to
    /// `gh repo view --web`, which resolves the page from that checkout's git
    /// remotes.
    Open {
        /// Select a repository even when standing in one
        #[arg(short, long)]
        select: bool,
    },
    /// Clone repositories from GitHub into your root
    ///
    /// With no argument, select an owner — suggested from your config, the owners
    /// already under your root, and your own GitHub account — or type any other.
    /// Then fuzzy-select one or more of that owner's repositories; `tab` selects
    /// several and they clone concurrently. `owner/repo` skips both selectors.
    ///
    /// Everything lands at `<root>/<owner>/<repo>`, so a clone is in
    /// `scriv repo sel` immediately afterwards. Each row carries the tags that
    /// make a repository unusual — `private` in yellow, `internal` in magenta —
    /// and the date it was last pushed to. Repositories you already have are
    /// marked with a green tick and skipped rather than re-cloned. Archived
    /// repositories are left out unless you ask for them.
    Clone {
        /// `owner` to select from, or `owner/repo` to clone directly
        #[arg(value_name = "OWNER[/REPO]")]
        target: Option<String>,
        /// Maximum number of repositories to fetch for an owner
        #[arg(short = 'L', long, default_value_t = 1000)]
        limit: usize,
        /// Also list an owner's archived repositories
        #[arg(long)]
        archived: bool,
    },
}

#[derive(Subcommand)]
enum FileCmd {
    /// List known files
    #[command(visible_alias = "list")]
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
    ///
    /// Ordered like `repo sel`: what you have opened before comes first. The
    /// same list, and the same order, as `scriv edit --tracked`.
    Sel,
    /// Add a file; omit the path to select one from the current directory
    Add {
        /// File path to add
        file: Option<String>,
    },
    /// Remove a file; omit the path to choose interactively
    Rm {
        /// File path to remove
        file: Option<String>,
    },
    /// Remove tracked files that no longer exist
    ///
    /// Prints the entries pointing at nothing and asks before dropping them —
    /// the files themselves are already gone, so this only edits the list.
    /// `--yes` skips the question, which is also what a run with no terminal on
    /// stdin needs.
    Prune {
        /// Prune without asking
        #[arg(short, long)]
        yes: bool,
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
enum NoteCmd {
    /// List every note in the vault, most recently modified first
    ///
    /// Paths are absolute and one per line, so the listing pipes into whatever
    /// reads paths — `scriv note ls | xargs grep -l TODO`.
    #[command(visible_alias = "list")]
    Ls {
        /// Also show each note's label, its tags, and both dates
        ///
        /// Modified carries a time of day and created does not: a creation date
        /// may have come from front matter, which names a day.
        #[arg(long)]
        status: bool,
    },
    /// Fuzzy-select a note and print its absolute path
    Sel,
    /// Start a note and open it straight away
    ///
    /// No question is asked first: with no NAME the note is called after the
    /// date and time, to the minute, and renaming it is what the editor is
    /// already open for. A NAME with a `/` in it names a directory below the
    /// vault, which is created if it is not there, and one with no `.` in it
    /// gains `.md`.
    ///
    /// The file itself is left for the editor to write, so a note started and
    /// abandoned is one that never existed rather than an empty one in every
    /// listing after it.
    New {
        /// What to call it; omit for the date and time
        #[arg(value_name = "NAME")]
        name: Option<String>,
    },
    /// Open the one permanent scratch note
    ///
    /// Somewhere to put a thought without first deciding whether it is worth a
    /// note of its own, and somewhere to find it again afterwards. The same
    /// file every time: `[note] scratch`, or `scratch/scratch.md`.
    Scratch,
    /// Review the notes that were never really written, and delete them
    ///
    /// Offers three kinds and no more: a note with nothing in it, one still
    /// called `Untitled`, and one whose name has no letters in it and whose
    /// front matter gives it no title either. Each row says which it is.
    ///
    /// Nothing goes without being listed and then agreed to. `tab` selects
    /// several, the preview shows what is in each, and what you chose is
    /// printed before the question is asked.
    Cleanup {
        /// Delete without asking
        #[arg(short, long)]
        yes: bool,
    },
    /// Search inside every note, as you type
    ///
    /// The query goes to `ripgrep` rather than to the fuzzy matcher, so the
    /// list is every matching *line* in the vault and it is rebuilt on each
    /// keystroke. `ctrl-q` switches to filtering what came back.
    ///
    /// Matching is fuzzy — the letters you typed, in order, anywhere on the
    /// line — so `errhand` finds "error handling". `ctrl-x` searches for the
    /// query exactly instead, for a phrase or a snippet of code, and `ctrl-f`
    /// goes back; the header says which is in force.
    ///
    /// `tab` takes several. What you pick opens at its line, and anything else
    /// you picked lands in the quickfix list behind it — which needs a vim; any
    /// other editor is handed the files and no line numbers.
    Rg {
        /// Text to open the search with
        #[arg(value_name = "QUERY", allow_hyphen_values = true)]
        query: Option<String>,
    },
    /// Open notes; omit the names to select them
    ///
    /// `tab` selects several and they open together. A name is a path below the
    /// vault, exactly as `note ls` prints it.
    ///
    /// The command is `[note] editor`, falling back to `$VISUAL` then
    /// `$EDITOR`. It is a setting of its own because a note is as often read as
    /// written — `glow` and `nvim` are both answers. The fish integration binds
    /// this to f10.
    Edit {
        /// Notes to open; omit to select interactively
        #[arg(value_name = "NAME")]
        notes: Vec<String>,
    },
}

#[derive(Subcommand)]
enum BranchCmd {
    /// List local and remote branches
    #[command(visible_alias = "list")]
    Ls {
        /// Show the current-branch marker, local/both/remote tag, and last commit
        #[arg(long)]
        status: bool,
        #[command(flatten)]
        scope: BranchScope,
    },
    /// Fuzzy-select a branch and print its name
    Sel {
        #[command(flatten)]
        scope: BranchScope,
    },
    /// Check out a branch; omit the name to select one
    ///
    /// Checking out a remote-only branch creates the matching local branch and
    /// sets its upstream.
    #[command(visible_aliases = ["co", "switch"])]
    Checkout {
        /// Branch to check out, e.g. `main` or `origin/feature`
        branch: Option<String>,
        #[command(flatten)]
        scope: BranchScope,
    },
    /// Delete local branches; omit the names to select them
    ///
    /// `tab` selects several. Each is listed with whether git can see its
    /// commits have landed, and answering the question that follows is what
    /// lets an unmerged one go — a repository that squashes its merges has no
    /// other kind, so there is no flag to type every time and read never.
    ///
    /// Remote branches are never offered: deleting one is a push.
    Rm {
        /// Branches to delete; omit to select interactively
        #[arg(value_name = "BRANCH")]
        branches: Vec<String>,
        /// Delete without asking
        #[arg(short, long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum WorktreeCmd {
    /// List this repository's worktrees
    #[command(visible_alias = "list")]
    Ls {
        /// Return absolute file paths
        #[arg(short = 'A', long)]
        absolute_paths: bool,
        /// Show the current-worktree marker, what each has checked out, and
        /// whether it is locked or prunable
        #[arg(long)]
        status: bool,
    },
    /// Fuzzy-select a worktree and print its absolute path
    Sel,
    /// Add a worktree; omit the branch to select or name one
    ///
    /// A branch that does not exist is created, since a tree is usually where
    /// a piece of work starts. A remote-only one arrives tracking it.
    ///
    /// Where the tree goes is `[worktree] root` — `.worktrees` inside the
    /// repository by default, one directory per branch with `/` written as `-`.
    /// An absolute root holds the trees of every repository, under the
    /// repository's own name. The path is printed, so `cd (scriv worktree add
    /// feat/x)` lands in it.
    Add {
        /// Branch the tree checks out; omit to select or type one
        branch: Option<String>,
    },
    /// Remove worktrees; omit the paths to select them
    ///
    /// `tab` selects several. Neither the main tree nor the one you are
    /// standing in is offered — git will not remove either. The branches they
    /// had checked out are left alone; that is `scriv branch rm`.
    Rm {
        /// Worktrees to remove, by path; omit to select interactively
        #[arg(value_name = "PATH")]
        paths: Vec<String>,
        /// Remove a tree with uncommitted changes in it
        #[arg(short, long)]
        force: bool,
        /// Remove without asking
        #[arg(short, long)]
        yes: bool,
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
    #[command(visible_alias = "list")]
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
    Sel {
        #[command(flatten)]
        scope: PrScope,
    },
    /// Check out a pull request's branch; omit the number to select one
    #[command(visible_alias = "co")]
    Checkout {
        /// Pull request number
        number: Option<u64>,
        #[command(flatten)]
        scope: PrScope,
    },
    /// Open a pull request in the browser; omit the number to select one
    Open {
        /// Pull request number
        number: Option<u64>,
        /// Open the pull request for the checked-out branch, without asking
        ///
        /// A branch with none opens the repository's pull request list
        /// instead, which is also what a detached HEAD gets. The fish
        /// integration binds this to f2.
        #[arg(long, conflicts_with_all = ["number", "state", "limit"])]
        current: bool,
        #[command(flatten)]
        scope: PrScope,
    },
    /// Merge a pull request; omit the number to select one
    ///
    /// The selector colours each row by whether it can actually be merged — green
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

/// Narrow a `proc` listing to what holds a TCP port, shared by all three.
#[derive(clap::Args)]
struct PortScope {
    /// Only the process listening on this TCP port
    ///
    /// Answers "what is holding 3000" without a pid, through `lsof`. Listening
    /// sockets only: the process that owns the port, not everything talking
    /// to it.
    #[arg(short, long, value_name = "PORT")]
    port: Option<u16>,
}

#[derive(Subcommand)]
enum ProcCmd {
    /// List running processes, busiest first
    #[command(visible_alias = "list")]
    Ls {
        /// Also show the owner, CPU share and how long it has been running
        ///
        /// The plain listing is `<pid> <command>`, so `cut -d' ' -f1` reaches
        /// the pid without a parser.
        #[arg(long)]
        status: bool,
        #[command(flatten)]
        scope: PortScope,
    },
    /// Fuzzy-select a process and print its pid
    Sel {
        #[command(flatten)]
        scope: PortScope,
    },
    /// Signal processes; omit the pids to select them
    ///
    /// `tab` selects several in the selector and they are signalled together.
    /// The default signal is `TERM`, which a process can catch to flush its
    /// buffers and clean up after itself; `--force` sends `KILL`, which it
    /// cannot.
    ///
    /// `--port` opens no selector: a port names its processes as precisely as
    /// a pid does, and what it named is printed as they are signalled.
    Kill {
        /// Processes to signal, by pid; omit to select interactively
        #[arg(value_name = "PID")]
        pids: Vec<i32>,
        /// Signal to send, by name or number — `TERM`, `HUP`, `9`
        #[arg(short, long, value_name = "SIGNAL", default_value = "TERM")]
        signal: String,
        /// Send `KILL` instead, which cannot be caught or ignored
        #[arg(short = '9', long, conflicts_with = "signal")]
        force: bool,
        #[command(flatten)]
        scope: PortScope,
    },
}

#[derive(Subcommand)]
enum HistoryCmd {
    /// List past commands, most recent first
    #[command(visible_alias = "list")]
    Ls {
        /// Prefix each command with the local date and time it was last run
        ///
        /// Fixed width and `YYYY-MM-DD HH:MM`, so the column sorts and cuts.
        #[arg(long)]
        status: bool,
    },
    /// Fuzzy-select a past command and print it
    Sel {
        /// Text to open the search box with
        ///
        /// Takes a value beginning with `-` — the fish integration passes
        /// whatever is on the command line, and a half-typed `--version` is
        /// text to search for, not a flag. Without this, ctrl-r would fail
        /// before opening, silently, since a key binding has nowhere to
        /// report an error.
        #[arg(short, long, value_name = "TEXT", allow_hyphen_values = true)]
        query: Option<String>,
        /// End the printed command with a NUL rather than a newline
        ///
        /// A command may contain newlines of its own, so only a NUL tells the
        /// shell reading this where one ends. This is what the fish integration
        /// uses; `read --null` is the other half of it.
        #[arg(short = '0', long)]
        print0: bool,
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
    /// Check everything scriv depends on and report what is wrong
    ///
    /// Looks at the config file, the paths it names, the repositories
    /// discovery actually finds, your editor, `git`, `gh` and whether it is
    /// still logged in, fish's history file and the tracked-file list — all in
    /// one pass, each with what to do about it. Exits non-zero only when
    /// something is genuinely broken, so it is worth putting in a setup script;
    /// a warning still leaves scriv working.
    ///
    /// The login is asked of GitHub, so this is the one command here that waits
    /// on the network.
    Check,
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
        // A cancelled selector is a silent, conventional exit, not an error.
        Err(err) if err.is::<Cancelled>() => ExitCode::from(130),
        // git and gh explain their own failures; pass the status through.
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
    let ctx = Ctx::load(cli.config.as_deref(), cli.verbose, cli.color)?;

    match cli.command {
        Command::Repo { command } => match command {
            RepoCmd::Ls { absolute_paths } => cmd::repo::ls(&ctx, absolute_paths),
            RepoCmd::Sel => cmd::repo::sel(&ctx),
            RepoCmd::Open { select } => cmd::repo::open(&ctx, select),
            RepoCmd::Clone {
                target,
                limit,
                archived,
            } => cmd::repo::clone(&ctx, target.as_deref(), limit, archived),
        },
        Command::File { command } => match command {
            FileCmd::Ls {
                status,
                missing,
                exists,
            } => cmd::file::ls(&ctx, status, missing, exists),
            FileCmd::Sel => cmd::file::sel(&ctx),
            FileCmd::Add { file } => cmd::file::add(&ctx, file.as_deref()),
            FileCmd::Rm { file } => cmd::file::remove(&ctx, file.as_deref()),
            FileCmd::Prune { yes } => cmd::file::prune(&ctx, yes),
        },
        // No subcommand is `edit file`, dispatched through the same arm so
        // the two spellings cannot drift apart.
        Command::Edit {
            command,
            files,
            tracked,
        } => match command.unwrap_or(EditCmd::File { files, tracked }) {
            EditCmd::File { files, tracked } => cmd::edit::file(&ctx, &files, tracked),
            EditCmd::Dir { dirs } => cmd::edit::dir(&ctx, &dirs),
        },
        Command::Note { command } => match command {
            NoteCmd::Ls { status } => cmd::note::ls(&ctx, status),
            NoteCmd::Sel => cmd::note::sel(&ctx),
            NoteCmd::Edit { notes } => cmd::note::edit(&ctx, &notes),
            NoteCmd::New { name } => cmd::note::new(&ctx, name.as_deref()),
            NoteCmd::Rg { query } => cmd::note::rg(&ctx, query.as_deref()),
            NoteCmd::Scratch => cmd::note::scratch(&ctx),
            NoteCmd::Cleanup { yes } => cmd::note::cleanup(&ctx, yes),
        },
        Command::Branch { command } => match command {
            BranchCmd::Ls { status, scope } => {
                cmd::branch::ls(&ctx, scope.filter(), status, scope.fetch)
            }
            BranchCmd::Sel { scope } => cmd::branch::sel(&ctx, scope.filter(), scope.fetch),
            BranchCmd::Checkout { branch, scope } => {
                cmd::branch::checkout(&ctx, branch.as_deref(), scope.filter(), scope.fetch)
            }
            BranchCmd::Rm { branches, yes } => cmd::branch::rm(&ctx, &branches, yes),
        },
        Command::Worktree { command } => match command {
            WorktreeCmd::Ls {
                absolute_paths,
                status,
            } => cmd::worktree::ls(&ctx, absolute_paths, status),
            WorktreeCmd::Sel => cmd::worktree::sel(&ctx),
            WorktreeCmd::Add { branch } => cmd::worktree::add(&ctx, branch.as_deref()),
            WorktreeCmd::Rm { paths, force, yes } => {
                cmd::worktree::remove(&ctx, &paths, force, yes)
            }
        },
        Command::Pr { command } => match command {
            PrCmd::Ls { status, scope } => cmd::pr::ls(&ctx, &scope.state, scope.limit, status),
            PrCmd::Sel { scope } => cmd::pr::sel(&ctx, &scope.state, scope.limit),
            PrCmd::Checkout { number, scope } => {
                cmd::pr::checkout(&ctx, number, &scope.state, scope.limit)
            }
            PrCmd::Open {
                number,
                current,
                scope,
            } => cmd::pr::open(&ctx, number, current, &scope.state, scope.limit),
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
        Command::Proc { command } => match command {
            ProcCmd::Ls { status, scope } => cmd::proc::ls(&ctx, status, scope.port),
            ProcCmd::Sel { scope } => cmd::proc::sel(&ctx, scope.port),
            ProcCmd::Kill {
                pids,
                signal,
                force,
                scope,
            } => {
                // `--force` and `--signal` conflict, so this is a choice
                // between the flag and the default.
                let signal = if force {
                    scriv::proc::Signal::KILL
                } else {
                    scriv::proc::Signal::parse(&signal)?
                };
                cmd::proc::kill(&ctx, &pids, signal, scope.port)
            }
        },
        Command::History { command } => match command {
            HistoryCmd::Ls { status } => cmd::history::ls(&ctx, status),
            HistoryCmd::Sel { query, print0 } => cmd::history::sel(&ctx, query.as_deref(), print0),
        },
        Command::Config { command } => match command {
            ConfigCmd::Init { force } => cmd::config::init(&ctx, force),
            ConfigCmd::Print => cmd::config::print(&ctx),
            ConfigCmd::Path => cmd::config::path(&ctx),
            ConfigCmd::Check => cmd::config::check(&ctx),
        },
        Command::Init { .. } => unreachable!("init handled before Ctx"),
    }
}
