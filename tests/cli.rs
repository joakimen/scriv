//! End-to-end tests of the `scriv` binary.
//!
//! These cover what the unit tests cannot: that a flag reaches the function it
//! names, that an error leaves the right exit status behind, and that stdout
//! carries what a shell is expected to read.
//!
//! **Every run is sealed off from the machine it runs on.** `HOME`,
//! `XDG_CONFIG_HOME`, `XDG_DATA_HOME` and `PWD` point at a temporary directory
//! and the inherited environment is otherwise wiped.
//!
//! Nothing here opens a selector: a test that allocated a pty would be testing
//! skim. The commands exercised are the ones a script can call.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use tempfile::TempDir;

/// The binary under test, as cargo built it for this run.
const BIN: &str = env!("CARGO_BIN_EXE_scriv");

/// A sealed-off scriv installation: its own home, config file and known-files
/// list, none of which outlive the test.
struct Sandbox {
    home: TempDir,
}

/// What one run of the binary came back with.
struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Run {
    fn ok(&self) -> &Self {
        assert_eq!(
            self.code,
            Some(0),
            "expected success\n--- stdout ---\n{}\n--- stderr ---\n{}",
            self.stdout,
            self.stderr
        );
        self
    }

    fn code(&self, expected: i32) -> &Self {
        assert_eq!(
            self.code,
            Some(expected),
            "wrong exit status\n--- stdout ---\n{}\n--- stderr ---\n{}",
            self.stdout,
            self.stderr
        );
        self
    }

    /// The lines of stdout, with the trailing blank one dropped.
    fn lines(&self) -> Vec<&str> {
        self.stdout.lines().collect()
    }
}

impl Sandbox {
    fn new() -> Self {
        Self {
            home: TempDir::new().expect("temp home"),
        }
    }

    fn home(&self) -> &Path {
        self.home.path()
    }

    fn config_path(&self) -> PathBuf {
        self.home().join(".config/scriv/config.toml")
    }

    /// The known-files list, which lives beside the config file.
    fn files_path(&self) -> PathBuf {
        self.home().join(".config/scriv/files")
    }

    fn write_config(&self, toml: &str) -> PathBuf {
        let path = self.config_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, toml).unwrap();
        path
    }

    /// Run `scriv` with `args`, from `cwd`. `env_clear` is the whole point: an
    /// inherited `SCRIV_CONFIG` or `HOME` would change what is being tested.
    fn run_in(&self, cwd: &Path, args: &[&str]) -> Run {
        self.run_full(cwd, args, &[])
    }

    fn run(&self, args: &[&str]) -> Run {
        self.run_full(self.home(), args, &[])
    }

    /// [`Sandbox::run`] with `env` set on top — for the variables scriv is
    /// meant to react to, such as `SCRIV_NO_COLOR`.
    fn run_with_env(&self, args: &[&str], env: &[(&str, &str)]) -> Run {
        self.run_full(self.home(), args, env)
    }

    /// [`Sandbox::run`] with `HOME` taken away — the one variable scriv cannot
    /// resolve a configuration path without.
    fn run_without_home(&self, args: &[&str]) -> Run {
        let mut cmd = self.command(self.home(), args);
        cmd.env_remove("HOME");
        finish(cmd)
    }

    fn run_full(&self, cwd: &Path, args: &[&str], env: &[(&str, &str)]) -> Run {
        let mut cmd = self.command(cwd, args);
        for (key, value) in env {
            cmd.env(key, value);
        }
        finish(cmd)
    }

    fn command(&self, cwd: &Path, args: &[&str]) -> Command {
        let mut cmd = Command::new(BIN);
        cmd.args(args)
            .current_dir(cwd)
            .env_clear()
            .env("HOME", self.home())
            .env("PWD", cwd)
            .env("XDG_CONFIG_HOME", self.home().join(".config"))
            .env("XDG_DATA_HOME", self.home().join(".local/share"))
            // `gh`, `git` and the editor are looked up on PATH.
            .env("PATH", std::env::var("PATH").unwrap_or_default());
        cmd
    }
}

fn finish(mut cmd: Command) -> Run {
    let out = cmd.output().expect("running scriv");
    Run {
        code: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

/// Create `<root>/<owner>/<name>` as something discovery will call a
/// repository.
fn mk_repo(root: &Path, owner: &str, name: &str) -> PathBuf {
    let path = root.join(owner).join(name);
    std::fs::create_dir_all(path.join(".git")).unwrap();
    path
}

// --- the argument surface ---------------------------------------------------

#[test]
fn help_lists_every_top_level_command() {
    let sandbox = Sandbox::new();
    let run = sandbox.run(&["--help"]);
    run.ok();
    for command in [
        "repo", "file", "edit", "branch", "worktree", "pr", "proc", "history", "config", "init",
    ] {
        assert!(
            run.stdout.contains(command),
            "`{command}` missing from --help:\n{}",
            run.stdout
        );
    }
    assert!(run.stdout.contains("Examples:"), "{}", run.stdout);
}

/// The shape is asserted rather than the exact string: both forms are correct
/// depending on where the build happened, and demanding one would fail in CI
/// or on a release runner.
#[test]
fn version_reports_the_crate_version_and_flags_a_development_build() {
    let sandbox = Sandbox::new();
    let run = sandbox.run(&["--version"]);
    run.ok();

    let version = run
        .stdout
        .trim()
        .strip_prefix("scriv ")
        .unwrap_or_else(|| panic!("unexpected --version output: {}", run.stdout));
    let crate_version = env!("CARGO_PKG_VERSION");

    let Some(dev) = version.strip_prefix(crate_version) else {
        panic!("`{version}` does not start with the crate version {crate_version}");
    };
    assert!(
        dev.is_empty() || dev.starts_with("-dev."),
        "`{version}` is neither the release version nor a development build",
    );
    if let Some(rest) = dev.strip_prefix("-dev.") {
        let sha = rest.strip_suffix(".dirty").unwrap_or(rest);
        assert!(
            !sha.is_empty() && sha.chars().all(|c| c.is_ascii_hexdigit()),
            "`{version}` names no commit",
        );
    }
}

#[test]
fn an_unknown_command_is_a_usage_error() {
    let sandbox = Sandbox::new();
    let run = sandbox.run(&["reop", "ls"]);
    run.code(2);
    assert!(run.stdout.is_empty(), "usage errors belong on stderr");
}

#[test]
fn help_names_the_aliases_the_binary_accepts() {
    let sandbox = Sandbox::new();
    let top = sandbox.run(&["--help"]);
    top.ok();
    for alias in ["r", "f", "e", "b", "w", "pc", "h", "c"] {
        assert!(
            top.stdout.contains(&format!("[aliases: {alias}]")),
            "`{alias}` is accepted but unmentioned:\n{}",
            top.stdout
        );
    }

    let branch = sandbox.run(&["branch", "--help"]);
    branch.ok();
    assert!(
        branch.stdout.contains("[aliases: co, switch]"),
        "{}",
        branch.stdout
    );
}

#[test]
fn the_help_examples_stay_at_three() {
    let sandbox = Sandbox::new();
    let run = sandbox.run(&["--help"]);
    run.ok();
    let examples = run
        .stdout
        .split_once("Examples:")
        .expect("no examples block")
        .1;
    let lines = examples.lines().filter(|l| !l.trim().is_empty()).count();
    assert_eq!(lines, 3, "the examples block has grown:{examples}");
}

#[test]
fn the_documented_abbreviations_resolve() {
    let sandbox = Sandbox::new();
    for args in [
        &["c", "--help"][..],
        &["r", "--help"][..],
        &["e", "--help"][..],
        &["b", "--help"][..],
        &["w", "--help"][..],
        &["f", "--help"][..],
        &["h", "--help"][..],
        &["pc", "--help"][..],
        &["branch", "co", "--help"][..],
        &["repo", "list", "--help"][..],
    ] {
        sandbox.run(args).ok();
    }
}

/// `$EDITOR` is `echo` here, so what it was asked to open comes back on
/// stdout. The `--` is part of the contract: to vim, `-c` is an Ex command.
#[test]
fn edit_defaults_to_the_file_subcommand() {
    let sandbox = Sandbox::new();
    let bare = sandbox.run_with_env(&["edit", "notes.md"], &[("EDITOR", "echo")]);
    bare.ok();
    let explicit = sandbox.run_with_env(&["edit", "file", "notes.md"], &[("EDITOR", "echo")]);
    explicit.ok();
    assert_eq!(bare.stdout, explicit.stdout, "the two spellings disagree");
    assert_eq!(bare.stdout.trim(), "-- notes.md");
}

#[test]
fn edit_dir_opens_the_directories_it_is_given() {
    let sandbox = Sandbox::new();
    let run = sandbox.run_with_env(&["edit", "dir", "src", "tests"], &[("EDITOR", "echo")]);
    run.ok();
    assert_eq!(run.stdout.trim(), "-- src tests");
}

#[test]
fn every_registry_exposes_sel() {
    let sandbox = Sandbox::new();
    for group in [
        "repo", "file", "branch", "worktree", "pr", "proc", "history",
    ] {
        sandbox.run(&[group, "sel", "--help"]).ok();
    }
}

/// The run gets no terminal, so it fails at the selector rather than at
/// parsing: "needs a terminal" is the proof the argument was accepted.
#[test]
fn a_history_query_may_begin_with_a_dash() {
    let sandbox = Sandbox::new();
    let history = sandbox.home().join(".local/share/fish/fish_history");
    std::fs::create_dir_all(history.parent().unwrap()).unwrap();
    std::fs::write(&history, "- cmd: git status\n  when: 1700000000\n").unwrap();

    let run = sandbox.run(&["history", "sel", "--query", "--version"]);
    assert!(
        !run.stderr.contains("a value is required"),
        "the query was read as a flag: {}",
        run.stderr
    );
    assert!(
        run.stderr.contains("needs a terminal"),
        "expected to get as far as the selector: {}",
        run.stderr
    );
}

#[test]
fn repo_clone_refuses_a_slug_github_could_not_have_issued() {
    let sandbox = Sandbox::new();
    sandbox.write_config(&format!(
        "[repo]\nroot = {:?}\n",
        sandbox.home().join("dev").to_str().unwrap()
    ));
    for slug in ["../../etc/passwd", "acme/../../etc"] {
        let run = sandbox.run(&["repo", "clone", slug]);
        run.code(1);
        assert!(
            run.stderr.contains("not a GitHub owner"),
            "{slug} was accepted: {}",
            run.stderr
        );
    }
}

// --- config -----------------------------------------------------------------

#[test]
fn config_init_writes_a_config_the_next_command_accepts() {
    let sandbox = Sandbox::new();
    let init = sandbox.run(&["config", "init"]);
    init.ok();
    assert!(sandbox.config_path().exists(), "{}", init.stdout);

    let print = sandbox.run(&["config", "print"]);
    print.ok();
    assert!(print.stdout.contains("[repo]"), "{}", print.stdout);
    assert!(print.stdout.contains("root:"), "{}", print.stdout);
}

#[test]
fn config_init_refuses_to_clobber_without_force() {
    let sandbox = Sandbox::new();
    sandbox.write_config("[repo]\nroot = \"~/mine\"\n");

    let again = sandbox.run(&["config", "init"]);
    again.code(1);
    assert!(again.stderr.contains("already exists"), "{}", again.stderr);
    // Untouched.
    let kept = std::fs::read_to_string(sandbox.config_path()).unwrap();
    assert!(kept.contains("~/mine"), "the edited config was overwritten");

    sandbox.run(&["config", "init", "--force"]).ok();
    let replaced = std::fs::read_to_string(sandbox.config_path()).unwrap();
    assert!(!replaced.contains("~/mine"), "--force did not overwrite");
}

#[test]
fn config_path_prints_one_bare_path() {
    let sandbox = Sandbox::new();
    let run = sandbox.run(&["config", "path"]);
    run.ok();
    assert_eq!(run.lines(), vec![sandbox.config_path().to_str().unwrap()]);
}

/// Every path scriv resolves hangs off the home directory, and `$HOME` is the
/// only place it is read from. Without it the run has to stop and say so,
/// rather than carry on against a guess from the passwd file.
#[test]
fn no_home_fails_by_name() {
    let sandbox = Sandbox::new();
    let run = sandbox.run_without_home(&["config", "path"]);
    run.code(1);
    assert!(run.stderr.contains("HOME"), "{}", run.stderr);
}

#[test]
fn the_config_flag_points_at_another_file() {
    let sandbox = Sandbox::new();
    let elsewhere = sandbox.home().join("elsewhere.toml");
    std::fs::write(&elsewhere, "[repo]\nroot = \"~/somewhere-else\"\n").unwrap();

    let run = sandbox.run(&["--config", elsewhere.to_str().unwrap(), "config", "print"]);
    run.ok();
    assert!(run.stdout.contains("somewhere-else"), "{}", run.stdout);
}

#[test]
fn a_legacy_config_fails_with_the_replacement_written_out() {
    let sandbox = Sandbox::new();
    sandbox.write_config("[[paths]]\npath = \"~/dev/github.com/acme\"\ndepth = 1\n");

    let run = sandbox.run(&["config", "print"]);
    run.code(1);
    assert!(run.stderr.contains("old `paths` format"), "{}", run.stderr);
    assert!(run.stderr.contains("[repo]"), "{}", run.stderr);
    assert!(
        run.stderr.contains("root = \"~/dev/github.com\""),
        "{}",
        run.stderr
    );
}

#[test]
fn a_broken_config_names_the_file() {
    let sandbox = Sandbox::new();
    let path = sandbox.write_config("[repo\nroot =");

    let run = sandbox.run(&["config", "print"]);
    run.code(1);
    assert!(
        run.stderr.contains(path.to_str().unwrap()),
        "the error did not say which file: {}",
        run.stderr
    );
}

#[test]
fn config_check_passes_on_a_sound_setup() {
    let sandbox = Sandbox::new();
    mk_repo(&sandbox.home().join("dev/github.com"), "acme", "billing");
    sandbox.write_config("[repo]\nroot = \"~/dev/github.com\"\n");

    let run = sandbox.run(&["config", "check"]);
    run.ok();
    assert!(run.stdout.contains("repo root"), "{}", run.stdout);
    // Discovery is really run: the count is what answers "is my root right".
    assert!(run.stdout.contains("1 found"), "{}", run.stdout);
    assert!(!run.stdout.contains('✗'), "{}", run.stdout);
}

#[test]
fn config_check_fails_on_a_missing_root() {
    let sandbox = Sandbox::new();
    sandbox.write_config("[repo]\nroot = \"~/not/here\"\n");

    let run = sandbox.run(&["config", "check"]);
    run.code(1);
    assert!(
        run.stdout
            .lines()
            .any(|l| l.starts_with('✗') && l.contains("repo root")),
        "{}",
        run.stdout
    );
    assert!(run.stderr.contains("checks failed"), "{}", run.stderr);
    assert!(!run.stdout.contains('\x1b'), "colour through a pipe");
}

/// `config check` and `--color` were written in parallel, each green on its
/// own branch, and the merge did not compile. Nothing tied the two together.
#[test]
fn config_check_honours_the_color_flag() {
    let sandbox = Sandbox::new();
    sandbox.write_config("[repo]\nroot = \"~/not/here\"\n");

    let forced = sandbox.run(&["--color", "always", "config", "check"]);
    forced.code(1);
    assert!(
        forced.stdout.contains('\x1b'),
        "the report ignored --color always: {:?}",
        forced.stdout
    );
}

#[test]
fn config_check_does_not_report_one_problem_twice() {
    let sandbox = Sandbox::new();
    sandbox.write_config("[repo]\nroot = \"~/not/here\"\n");

    let run = sandbox.run(&["config", "check"]);
    run.code(1);
    assert_eq!(
        run.stdout.lines().filter(|l| l.starts_with('✗')).count(),
        1,
        "{}",
        run.stdout
    );
    assert!(
        !run.stdout.contains("repositories"),
        "the repository count repeated the root failure: {}",
        run.stdout
    );
}

// --- repo discovery ---------------------------------------------------------

#[test]
fn repo_ls_finds_repositories_under_the_root() {
    let sandbox = Sandbox::new();
    let root = sandbox.home().join("dev/github.com");
    mk_repo(&root, "acme", "billing");
    mk_repo(&root, "acme", "auth");
    mk_repo(&root, "me", "dotfiles");
    // Neither of these is a repository: no `.git`, and too deep respectively.
    std::fs::create_dir_all(root.join("acme/not-a-repo")).unwrap();
    mk_repo(&root, "acme/billing", "vendored");

    sandbox.write_config("[repo]\nroot = \"~/dev/github.com\"\n");
    let run = sandbox.run(&["repo", "ls"]);
    run.ok();

    assert_eq!(
        run.lines(),
        vec![
            "~/dev/github.com/acme/auth",
            "~/dev/github.com/acme/billing",
            "~/dev/github.com/me/dotfiles",
        ],
        "stderr: {}",
        run.stderr
    );
}

#[test]
fn repo_ls_absolute_paths_are_absolute() {
    let sandbox = Sandbox::new();
    let root = sandbox.home().join("dev/github.com");
    mk_repo(&root, "acme", "billing");
    sandbox.write_config("[repo]\nroot = \"~/dev/github.com\"\n");

    let run = sandbox.run(&["repo", "ls", "-A"]);
    run.ok();
    assert_eq!(run.lines().len(), 1);
    assert!(
        Path::new(run.lines()[0]).is_absolute(),
        "{}",
        run.lines()[0]
    );
    assert!(!run.lines()[0].starts_with('~'));
}

#[test]
fn repo_ls_skips_ignored_directories() {
    let sandbox = Sandbox::new();
    let root = sandbox.home().join("dev/github.com");
    mk_repo(&root, "acme", "billing");
    mk_repo(&root, "node_modules", "vendored");
    sandbox.write_config("[repo]\nroot = \"~/dev/github.com\"\nignore = [\"node_modules\"]\n");

    let run = sandbox.run(&["repo", "ls"]);
    run.ok();
    assert_eq!(run.lines(), vec!["~/dev/github.com/acme/billing"]);
}

#[test]
fn repo_ls_without_a_root_says_what_to_do() {
    let sandbox = Sandbox::new();
    sandbox.write_config("[selector]\nheight = \"40%\"\n");

    let run = sandbox.run(&["repo", "ls"]);
    run.code(1);
    assert!(run.stderr.contains("root"), "{}", run.stderr);
    assert!(run.stderr.contains("config init"), "{}", run.stderr);
}

#[test]
fn repo_ls_reports_a_missing_root() {
    let sandbox = Sandbox::new();
    sandbox.write_config("[repo]\nroot = \"~/not/here\"\n");

    let run = sandbox.run(&["repo", "ls"]);
    run.code(1);
    assert!(run.stderr.contains("not/here"), "{}", run.stderr);
}

// --- worktrees --------------------------------------------------------------

/// A real repository with one linked worktree, since `scriv worktree` reads
/// git rather than the filesystem.
///
/// `git worktree add` needs a commit to point the new tree at, and the sandbox
/// has no git identity to make one with — hence the author and committer in
/// the environment, and the two `GIT_CONFIG_*` variables that keep the machine
/// running the test out of the result.
fn mk_worktree_repo(root: &Path) -> (PathBuf, PathBuf) {
    let main = root.join("scriv");
    std::fs::create_dir_all(&main).unwrap();

    for args in [
        &["init", "-b", "main"][..],
        &["commit", "--allow-empty", "-m", "init"][..],
        &["worktree", "add", "-b", "feat", "../feat"][..],
    ] {
        let out = Command::new("git")
            .args(args)
            .current_dir(&main)
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", root)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_AUTHOR_NAME", "scriv tests")
            .env("GIT_AUTHOR_EMAIL", "tests@example.invalid")
            .env("GIT_COMMITTER_NAME", "scriv tests")
            .env("GIT_COMMITTER_EMAIL", "tests@example.invalid")
            .output()
            .expect("running git");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr),
        );
    }

    (main, root.join("feat"))
}

/// git reports the real path of a worktree, and a temporary directory on macOS
/// is reached through a symlink.
fn real(path: &Path) -> String {
    path.canonicalize().unwrap().to_string_lossy().into_owned()
}

#[test]
fn worktree_ls_lists_the_main_tree_and_the_linked_one() {
    let sandbox = Sandbox::new();
    let (main, feat) = mk_worktree_repo(sandbox.home());

    let run = sandbox.run_in(&main, &["worktree", "ls", "--absolute-paths"]);
    run.ok();
    assert_eq!(run.lines(), vec![real(&main), real(&feat)]);
}

/// Which tree is current is the whole question the selector answers, and it is
/// the one the shell is standing in — not the one git happens to list first.
#[test]
fn worktree_ls_status_marks_the_tree_the_shell_is_in() {
    let sandbox = Sandbox::new();
    let (main, feat) = mk_worktree_repo(sandbox.home());

    let from_linked = sandbox.run_in(&feat, &["worktree", "ls", "--status", "-A"]);
    from_linked.ok();
    let lines = from_linked.lines();
    assert!(lines[0].starts_with("  main "), "{lines:?}");
    assert!(lines[1].starts_with("* feat "), "{lines:?}");
    assert!(lines[1].ends_with(&real(&feat)), "{lines:?}");

    let from_main = sandbox.run_in(&main, &["worktree", "ls", "--status", "-A"]);
    from_main.ok();
    assert!(
        from_main.lines()[0].starts_with("* main "),
        "{:?}",
        from_main.lines()
    );
}

/// `~` is for reading, `-A` is for piping. `HOME` is overridden with its
/// resolved form because git reports resolved paths, and a `~` is only
/// recognisable in one that shares its prefix.
#[test]
fn worktree_ls_collapses_home_unless_asked_for_absolute_paths() {
    let sandbox = Sandbox::new();
    let (main, feat) = mk_worktree_repo(sandbox.home());
    let home = sandbox.home().canonicalize().unwrap();
    let env = [("HOME", home.to_str().unwrap())];

    let collapsed = sandbox.run_full(&main, &["worktree", "ls"], &env);
    collapsed.ok();
    assert_eq!(collapsed.lines(), vec!["~/scriv", "~/feat"]);

    let absolute = sandbox.run_full(&main, &["worktree", "ls", "-A"], &env);
    absolute.ok();
    assert_eq!(absolute.lines(), vec![real(&main), real(&feat)]);
}

#[test]
fn worktree_ls_outside_a_repository_says_so() {
    let sandbox = Sandbox::new();
    let run = sandbox.run(&["worktree", "ls"]);
    run.code(1);
    assert!(
        run.stderr.contains("not inside a git repository"),
        "{}",
        run.stderr
    );
}

// --- the known-files list ---------------------------------------------------

#[test]
fn file_add_ls_and_remove_round_trip() {
    let sandbox = Sandbox::new();
    let note = sandbox.home().join("notes.md");
    std::fs::write(&note, "hello").unwrap();

    sandbox.run(&["file", "add", note.to_str().unwrap()]).ok();
    // Stored `~`-collapsed, so the list survives a different machine.
    let stored = std::fs::read_to_string(sandbox.files_path()).unwrap();
    assert_eq!(stored.trim(), "~/notes.md");

    // Listed expanded, because a listing is for handing to another command.
    let ls = sandbox.run(&["file", "ls"]);
    ls.ok();
    assert_eq!(ls.lines(), vec![note.to_str().unwrap()]);

    // A newline would come back as two entries `file rm` can never match.
    let bad = sandbox.run(&["file", "add", "one\ntwo.md"]);
    bad.code(1);
    assert!(bad.stderr.contains("one path per line"), "{}", bad.stderr);
    assert_eq!(
        sandbox.run(&["file", "ls"]).ok().lines(),
        vec![note.to_str().unwrap()],
        "the rejected path reached the list anyway"
    );

    let removed = sandbox.run(&["file", "rm", note.to_str().unwrap()]);
    removed.ok();
    assert!(removed.stdout.contains("Removed"), "{}", removed.stdout);
    assert!(sandbox.run(&["file", "ls"]).ok().lines().is_empty());
}

#[test]
fn file_add_refuses_a_duplicate() {
    let sandbox = Sandbox::new();
    let note = sandbox.home().join("notes.md");
    std::fs::write(&note, "hello").unwrap();

    sandbox.run(&["file", "add", note.to_str().unwrap()]).ok();
    let again = sandbox.run(&["file", "add", note.to_str().unwrap()]);
    again.code(1);
    assert!(again.stderr.contains("already exists"), "{}", again.stderr);
}

#[test]
fn file_ls_separates_present_from_missing() {
    let sandbox = Sandbox::new();
    let here = sandbox.home().join("here.md");
    std::fs::write(&here, "").unwrap();
    let gone = sandbox.home().join("gone.md");

    sandbox.run(&["file", "add", here.to_str().unwrap()]).ok();
    // Adding something absent warns but records it: the file may come back.
    let added = sandbox.run(&["file", "add", gone.to_str().unwrap()]);
    added.ok();
    assert!(added.stderr.contains("does not exist"), "{}", added.stderr);

    let missing = sandbox.run(&["file", "ls", "--missing"]);
    missing.ok();
    assert_eq!(missing.lines(), vec![gone.to_str().unwrap()]);

    let exists = sandbox.run(&["file", "ls", "--exists"]);
    exists.ok();
    assert_eq!(exists.lines(), vec![here.to_str().unwrap()]);

    let status = sandbox.run(&["file", "ls", "--status"]);
    status.ok();
    assert!(status.stdout.contains("✓ "), "{}", status.stdout);
    assert!(status.stdout.contains("✗ "), "{}", status.stdout);
}

#[test]
fn file_prune_drops_the_entries_whose_files_are_gone() {
    let sandbox = Sandbox::new();
    let here = sandbox.home().join("here.md");
    std::fs::write(&here, "").unwrap();
    let gone = sandbox.home().join("gone.md");
    std::fs::write(&gone, "").unwrap();

    sandbox.run(&["file", "add", here.to_str().unwrap()]).ok();
    sandbox.run(&["file", "add", gone.to_str().unwrap()]).ok();
    std::fs::remove_file(&gone).unwrap();

    let run = sandbox.run(&["file", "prune", "--yes"]);
    run.ok();
    assert!(run.stdout.contains("Removed"), "{}", run.stdout);
    assert!(run.stdout.contains("gone.md"), "{}", run.stdout);

    let left = sandbox.run(&["file", "ls"]);
    left.ok();
    assert_eq!(left.lines(), vec![here.to_str().unwrap()]);
}

#[test]
fn file_prune_refuses_to_assume_an_answer_it_could_not_ask_for() {
    let sandbox = Sandbox::new();
    let gone = sandbox.home().join("gone.md");
    std::fs::write(&gone, "").unwrap();
    sandbox.run(&["file", "add", gone.to_str().unwrap()]).ok();
    std::fs::remove_file(&gone).unwrap();

    // The test harness gives the child no terminal, which is exactly the case.
    let run = sandbox.run(&["file", "prune"]);
    run.code(1);
    assert!(run.stderr.contains("--yes"), "{}", run.stderr);

    // And the list is untouched.
    assert_eq!(sandbox.run(&["file", "ls"]).ok().lines().len(), 1);
}

#[test]
fn file_prune_says_so_when_there_is_nothing_to_do() {
    let sandbox = Sandbox::new();
    let here = sandbox.home().join("here.md");
    std::fs::write(&here, "").unwrap();
    sandbox.run(&["file", "add", here.to_str().unwrap()]).ok();

    // No `--yes`, and no terminal: reaching the confirmation at all would fail.
    let run = sandbox.run(&["file", "prune"]);
    run.ok();
    assert!(run.stdout.contains("Nothing to prune"), "{}", run.stdout);
    assert_eq!(sandbox.run(&["file", "ls"]).ok().lines().len(), 1);
}

#[test]
fn file_prune_shows_the_entries_before_removing_them() {
    let sandbox = Sandbox::new();
    let gone = sandbox.home().join("gone.md");
    std::fs::write(&gone, "").unwrap();
    sandbox.run(&["file", "add", gone.to_str().unwrap()]).ok();
    std::fs::remove_file(&gone).unwrap();

    let run = sandbox.run(&["file", "prune", "--yes"]);
    run.ok();
    let shown = run.stdout.find("✗ ").expect("no listing of what would go");
    let removed = run
        .stdout
        .find("Removed")
        .expect("nothing reported removed");
    assert!(shown < removed, "listed the entries after removing them");
}

#[test]
fn file_add_resolves_a_relative_path_against_the_working_directory() {
    let sandbox = Sandbox::new();
    let project = sandbox.home().join("project");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("main.rs"), "").unwrap();

    sandbox.run_in(&project, &["file", "add", "main.rs"]).ok();
    let stored = std::fs::read_to_string(sandbox.files_path()).unwrap();
    assert_eq!(stored.trim(), "~/project/main.rs");
}

// --- colour -----------------------------------------------------------------

/// A sandbox with one present and one missing tracked file, so `file ls
/// --status` has both a green row and a red one to colour.
fn coloured_listing(sandbox: &Sandbox) {
    let here = sandbox.home().join("here.md");
    std::fs::write(&here, "").unwrap();
    sandbox.run(&["file", "add", here.to_str().unwrap()]).ok();
    sandbox
        .run(&[
            "file",
            "add",
            sandbox.home().join("gone.md").to_str().unwrap(),
        ])
        .ok();
}

#[test]
fn color_always_colours_a_pipe_and_never_does_not() {
    let sandbox = Sandbox::new();
    coloured_listing(&sandbox);

    let always = sandbox.run(&["--color", "always", "file", "ls", "--status"]);
    always.ok();
    assert!(
        always.stdout.contains('\x1b'),
        "no colour through a pipe: {:?}",
        always.stdout
    );

    for args in [
        &["--color", "never", "file", "ls", "--status"][..],
        // The default through a pipe: unchanged from before there was a flag.
        &["file", "ls", "--status"][..],
    ] {
        let run = sandbox.run(args);
        run.ok();
        assert!(
            !run.stdout.contains('\x1b'),
            "{args:?} coloured: {:?}",
            run.stdout
        );
        // The glyphs are shapes, so the listing says as much either way.
        assert!(run.stdout.contains("✓ "), "{:?}", run.stdout);
        assert!(run.stdout.contains("✗ "), "{:?}", run.stdout);
    }
}

#[test]
fn scriv_no_color_turns_colour_off() {
    let sandbox = Sandbox::new();
    coloured_listing(&sandbox);

    let run = sandbox.run_with_env(
        &["--color", "auto", "file", "ls", "--status"],
        &[("SCRIV_NO_COLOR", "1")],
    );
    run.ok();
    assert!(!run.stdout.contains('\x1b'), "{:?}", run.stdout);

    // Set but empty is not set, as every variable of this shape works.
    let empty = sandbox.run_with_env(
        &["--color", "always", "file", "ls", "--status"],
        &[("SCRIV_NO_COLOR", "")],
    );
    empty.ok();
    assert!(empty.stdout.contains('\x1b'), "{:?}", empty.stdout);
}

#[test]
fn an_explicit_color_choice_outranks_no_color() {
    let sandbox = Sandbox::new();
    coloured_listing(&sandbox);

    let forced = sandbox.run_with_env(
        &["--color", "always", "file", "ls", "--status"],
        &[("SCRIV_NO_COLOR", "1")],
    );
    forced.ok();
    assert!(
        forced.stdout.contains('\x1b'),
        "SCRIV_NO_COLOR beat `--color always`: {:?}",
        forced.stdout
    );
}

#[test]
fn color_takes_only_the_three_conventional_values() {
    let sandbox = Sandbox::new();
    sandbox.run(&["--color", "sometimes", "file", "ls"]).code(2);
}

// --- processes --------------------------------------------------------------

#[test]
fn proc_ls_leads_every_row_with_a_pid() {
    let sandbox = Sandbox::new();
    let run = sandbox.run(&["proc", "ls"]);
    run.ok();
    let lines = run.lines();
    assert!(!lines.is_empty(), "listed no processes at all");
    for line in &lines {
        let (pid, command) = line.split_once(' ').unwrap_or_else(|| panic!("{line:?}"));
        pid.parse::<i32>()
            .unwrap_or_else(|_| panic!("not a pid: {line:?}"));
        assert!(!command.is_empty(), "{line:?}");
    }
}

#[test]
fn proc_ls_offers_neither_scriv_nor_the_process_that_ran_it() {
    let sandbox = Sandbox::new();
    let run = sandbox.run(&["proc", "ls"]);
    run.ok();
    let own = std::process::id().to_string();
    for line in run.lines() {
        let pid = line.split(' ').next().unwrap();
        assert_ne!(pid, own, "offered the process that ran it: {line:?}");
    }
}

#[test]
fn proc_ls_status_adds_columns_ahead_of_the_command() {
    let sandbox = Sandbox::new();
    let plain = sandbox.run(&["proc", "ls"]);
    let status = sandbox.run(&["proc", "ls", "--status"]);
    plain.ok();
    status.ok();
    let widest_plain = plain.lines().iter().map(|l| l.len()).max().unwrap_or(0);
    let widest_status = status.lines().iter().map(|l| l.len()).max().unwrap_or(0);
    assert!(
        widest_status > widest_plain,
        "--status added nothing:\n{}",
        status.stdout
    );
}

#[test]
fn proc_ls_status_is_pipe_safe_by_default() {
    let sandbox = Sandbox::new();
    let run = sandbox.run(&["proc", "ls", "--status"]);
    run.ok();
    assert!(!run.stdout.contains('\x1b'), "coloured a pipe");
    let forced = sandbox.run(&["--color", "always", "proc", "ls", "--status"]);
    forced.ok();
    assert!(forced.stdout.contains('\x1b'), "--color always did nothing");
}

#[test]
fn proc_kill_refuses_an_unknown_signal_before_selecting() {
    let sandbox = Sandbox::new();
    let run = sandbox.run(&["proc", "kill", "--signal", "HUPP"]);
    run.code(1);
    assert!(run.stderr.contains("unknown signal"), "{}", run.stderr);
    assert!(run.stderr.contains("known signals are"), "{}", run.stderr);
}

/// To `kill`, `0` and a negative number are process *groups*. Run for real,
/// with a signal that would work, so the refusal is what the test proves.
#[test]
fn proc_kill_refuses_a_pid_that_is_really_a_process_group() {
    let sandbox = Sandbox::new();
    for pid in ["0", "-1"] {
        let run = sandbox.run(&["proc", "kill", "--signal", "CONT", "--", pid]);
        run.code(1);
        assert!(
            run.stderr.contains("refusing to signal"),
            "{pid} was not refused: {}",
            run.stderr
        );
        assert!(run.stdout.is_empty(), "{pid} was signalled: {}", run.stdout);
    }
}

#[test]
fn proc_kill_still_accepts_an_ordinary_pid() {
    let sandbox = Sandbox::new();
    // What matters is that scriv got as far as asking `kill`.
    let run = sandbox.run(&["proc", "kill", "--signal", "CONT", "2147483646"]);
    assert!(
        !run.stderr.contains("refusing to signal"),
        "an ordinary pid was refused: {}",
        run.stderr
    );
}

#[test]
fn proc_kill_will_not_take_a_signal_and_force_at_once() {
    let sandbox = Sandbox::new();
    sandbox
        .run(&["proc", "kill", "--signal", "TERM", "--force"])
        .code(2);
}

/// Uses a child of the test's own making, so nothing on the machine is at
/// risk.
#[test]
fn proc_kill_signals_the_pid_it_is_given() {
    let sandbox = Sandbox::new();
    let mut child = Command::new("sleep")
        .arg("60")
        .spawn()
        .expect("spawning a process to kill");
    let pid = child.id().to_string();

    let run = sandbox.run(&["proc", "kill", &pid]);
    run.ok();
    assert!(
        run.stdout.contains("sent TERM to"),
        "did not report the signal:\n{}",
        run.stdout
    );

    let status = child.wait().expect("waiting for the killed process");
    assert!(
        !status.success(),
        "the process outlived the signal: {status:?}"
    );
}

#[test]
fn proc_kill_force_sends_kill() {
    let sandbox = Sandbox::new();
    let mut child = Command::new("sleep")
        .arg("60")
        .spawn()
        .expect("spawning a process to kill");
    let pid = child.id().to_string();

    let run = sandbox.run(&["proc", "kill", "--force", &pid]);
    run.ok();
    assert!(run.stdout.contains("sent KILL to"), "{}", run.stdout);
    child.wait().expect("waiting for the killed process");
}

#[test]
fn proc_kill_passes_a_failure_through() {
    let sandbox = Sandbox::new();
    let mut child = Command::new("sleep").arg("0").spawn().expect("spawning");
    let pid = child.id().to_string();
    child.wait().expect("reaping");

    let run = sandbox.run(&["proc", "kill", &pid]);
    assert_ne!(run.code, Some(0), "signalled a process that had exited");
    assert!(
        !run.stderr.contains("error: "),
        "restated what kill already said:\n{}",
        run.stderr
    );
}

// --- fish history -----------------------------------------------------------

/// Write a fish history file where scriv looks for one by default.
fn write_history(sandbox: &Sandbox, body: &str) {
    let path = sandbox.home().join(".local/share/fish/fish_history");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

#[test]
fn history_ls_is_newest_first_and_one_command_per_line() {
    let sandbox = Sandbox::new();
    write_history(
        &sandbox,
        "- cmd: cargo test\n  when: 100\n\
         - cmd: git status\n  when: 200\n\
         - cmd: cargo test\n  when: 300\n\
         - cmd: git commit -m 'a\\nb'\n  when: 400\n",
    );

    let run = sandbox.run(&["history", "ls"]);
    run.ok();
    assert_eq!(
        run.lines(),
        vec!["git commit -m 'a ⏎ b'", "cargo test", "git status"],
        "stderr: {}",
        run.stderr
    );
}

#[test]
fn history_ls_status_dates_every_row_in_one_column() {
    let sandbox = Sandbox::new();
    write_history(
        &sandbox,
        "- cmd: cargo test\n  when: 1785394626\n- cmd: undated\n",
    );

    let run = sandbox.run(&["history", "ls", "--status"]);
    run.ok();
    let lines = run.lines();
    // Newest first, and an entry with no `when:` sorts as it was stored.
    assert_eq!(lines.len(), 2, "{lines:?}");
    let (undated, dated) = (lines[0], lines[1]);

    assert!(
        dated.starts_with("2026-07-30 "),
        "not a local timestamp: {dated:?}"
    );
    // Same column on both rows, so `cut -c18-` and `sort` work.
    let column = |line: &str| line.len() - line.trim_start().len();
    assert_eq!(
        column(undated),
        "2026-07-30 13:57  ".len(),
        "an undated row did not hold the column open: {undated:?}"
    );
    assert!(undated.trim_end().ends_with("undated"), "{undated:?}");
}

#[test]
fn history_without_a_file_names_the_path_it_looked_at() {
    let sandbox = Sandbox::new();
    let run = sandbox.run(&["history", "ls"]);
    run.code(1);
    assert!(run.stderr.contains("fish_history"), "{}", run.stderr);
    assert!(run.stderr.contains("[history] file"), "{}", run.stderr);
}

#[test]
fn history_survives_invalid_utf8() {
    let sandbox = Sandbox::new();
    let path = sandbox.home().join(".local/share/fish/fish_history");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut bytes = b"- cmd: echo ".to_vec();
    bytes.extend_from_slice(&[0xff, 0xfe]);
    bytes.extend_from_slice(b"\n  when: 100\n- cmd: git status\n  when: 200\n");
    std::fs::write(&path, bytes).unwrap();

    let run = sandbox.run(&["history", "ls"]);
    run.ok();
    assert_eq!(run.lines().len(), 2, "{:?}", run.lines());
    assert_eq!(run.lines()[0], "git status");
}

/// `println!` answers a closed pipe with a panic. A short listing fits in the
/// pipe buffer and never triggers it, so the fixture is several times its
/// size.
#[test]
fn a_listing_ends_quietly_when_the_reader_stops_reading() {
    let sandbox = Sandbox::new();
    let mut history = String::new();
    // Past the 64 KiB pipe buffer, so the child is still writing when the read
    // end goes away.
    for i in 0..5000 {
        history.push_str(&format!(
            "- cmd: echo this is history entry number {i}\n  when: {}\n",
            1_700_000_000 + i
        ));
    }
    write_history(&sandbox, &history);

    let mut child = Command::new(BIN)
        .args(["history", "ls"])
        .current_dir(sandbox.home())
        .env_clear()
        .env("HOME", sandbox.home())
        .env("PWD", sandbox.home())
        .env("XDG_CONFIG_HOME", sandbox.home().join(".config"))
        .env("XDG_DATA_HOME", sandbox.home().join(".local/share"))
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawning scriv");

    // Read a little, then close the pipe — this is what `head` does.
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut buf = [0u8; 64];
    stdout.read_exact(&mut buf).expect("no rows at all");
    drop(stdout);

    let out = child.wait_with_output().expect("waiting for scriv");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("panicked"),
        "a closed pipe panicked:\n{stderr}"
    );
    assert_eq!(out.status.code(), Some(0), "stderr: {stderr}");
}

// --- shell integration ------------------------------------------------------

#[test]
fn init_fish_emits_functions_bindings_and_completions() {
    let sandbox = Sandbox::new();
    let run = sandbox.run(&["init", "fish"]);
    run.ok();
    for needle in [
        "function scriv-repo-cd",
        "function scriv-worktree-cd",
        "function scriv-history-select",
        "function fe",
        "function scriv_key_bindings",
        "complete -c scriv",
    ] {
        assert!(run.stdout.contains(needle), "missing {needle:?}");
    }
}

#[test]
fn init_emits_completions_for_the_other_shells() {
    let sandbox = Sandbox::new();
    for shell in ["bash", "zsh", "powershell", "elvish"] {
        let run = sandbox.run(&["init", shell]);
        run.ok();
        assert!(
            run.stdout.contains("scriv"),
            "{shell} completions were empty"
        );
        assert!(
            !run.stdout.contains("scriv_key_bindings"),
            "{shell} got fish's bindings"
        );
    }
}

#[test]
fn init_works_before_there_is_any_config() {
    let sandbox = Sandbox::new();
    assert!(!sandbox.config_path().exists());
    sandbox.run(&["init", "fish"]).ok();
}
