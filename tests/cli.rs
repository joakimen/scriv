//! End-to-end tests of the `scriv` binary.
//!
//! The unit tests under `src/` cover the decisions — how a ref is classified,
//! what a legacy config should be rewritten as, when a row is dated. What they
//! cannot cover is the wiring: that a flag reaches the function it names, that
//! an error leaves the right exit status behind, that stdout carries what a
//! shell is expected to read and stderr carries the rest. Every one of those is
//! a contract with whoever is typing, and none of them is visible from inside
//! the library.
//!
//! So these run the real binary, with a real config file and a real history
//! file, and read what comes back.
//!
//! **Every run is sealed off from the machine it runs on.** `HOME`,
//! `XDG_CONFIG_HOME`, `XDG_DATA_HOME` and `PWD` are all pointed at a temporary
//! directory and the inherited environment is otherwise wiped, so a test can
//! neither read the developer's own repositories, config and shell history nor
//! write to them. A test that passed only on a machine with a particular
//! `~/.config/scriv` would be worse than no test.
//!
//! Nothing here opens a picker: the interactive paths need a terminal, and a
//! test that allocated a pty would be testing skim. The commands exercised are
//! the ones a script can call — `ls`, `add`, `remove`, `config`, `init`.

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

    /// Run `scriv` with `args`, from `cwd`.
    ///
    /// `env_clear` is the whole point: an inherited `SCRIV_CONFIG`, `NO_COLOR`
    /// or `EDITOR` would quietly change what is being tested, and an inherited
    /// `HOME` would point the run at the developer's own config.
    fn run_in(&self, cwd: &Path, args: &[&str]) -> Run {
        let out = Command::new(BIN)
            .args(args)
            .current_dir(cwd)
            .env_clear()
            .env("HOME", self.home())
            .env("PWD", cwd)
            .env("XDG_CONFIG_HOME", self.home().join(".config"))
            .env("XDG_DATA_HOME", self.home().join(".local/share"))
            // `gh`, `git` and the editor are looked up on PATH; keep the real
            // one so the tests that reach them behave as they would in a shell.
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .output()
            .expect("running scriv");

        Run {
            code: out.status.code(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }

    fn run(&self, args: &[&str]) -> Run {
        self.run_in(self.home(), args)
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

/// `--help` is the documentation most users read, and clap prints it from the
/// same declarations that define the commands — so a command that exists and a
/// command that is documented cannot drift apart. What can drift is a command
/// vanishing from the enum entirely, which this notices.
#[test]
fn help_lists_every_top_level_command() {
    let sandbox = Sandbox::new();
    let run = sandbox.run(&["--help"]);
    run.ok();
    for command in [
        "repo", "file", "edit", "branch", "pr", "history", "config", "init",
    ] {
        assert!(
            run.stdout.contains(command),
            "`{command}` missing from --help:\n{}",
            run.stdout
        );
    }
    assert!(run.stdout.contains("Examples:"), "{}", run.stdout);
}

/// `--version` has to print the version cargo built, not a hardcoded string
/// that stops being true at the next release.
#[test]
fn version_reports_the_crate_version() {
    let sandbox = Sandbox::new();
    let run = sandbox.run(&["--version"]);
    run.ok();
    assert!(
        run.stdout.contains(env!("CARGO_PKG_VERSION")),
        "{}",
        run.stdout
    );
}

/// A mistyped command is a usage error, and the conventional status for one is
/// 2 — distinct from 1, which is the command running and failing. A script that
/// tells those apart deserves to be able to.
#[test]
fn an_unknown_command_is_a_usage_error() {
    let sandbox = Sandbox::new();
    let run = sandbox.run(&["reop", "ls"]);
    run.code(2);
    assert!(run.stdout.is_empty(), "usage errors belong on stderr");
}

/// Abbreviations are part of the surface people actually type. They are aliases
/// in clap, so nothing but a run of the binary proves they resolve.
#[test]
fn the_documented_abbreviations_resolve() {
    let sandbox = Sandbox::new();
    for args in [
        &["br", "--help"][..],
        &["hist", "--help"][..],
        &["e", "--help"][..],
        &["branch", "co", "--help"][..],
        &["repo", "list", "--help"][..],
    ] {
        sandbox.run(args).ok();
    }
}

// --- config -----------------------------------------------------------------

/// The starter config is what a new user gets, and the very next thing they do
/// is run something that reads it. Writing a template the tool then rejects is
/// the one failure that makes the first five minutes impossible.
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

/// A second `config init` must not silently overwrite an edited config —
/// `--force` is how you say you meant it.
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

/// `config path` exists to be read by something else — a shell function, an
/// editor command — so it prints the path and nothing else.
#[test]
fn config_path_prints_one_bare_path() {
    let sandbox = Sandbox::new();
    let run = sandbox.run(&["config", "path"]);
    run.ok();
    assert_eq!(run.lines(), vec![sandbox.config_path().to_str().unwrap()]);
}

/// `--config` has to reach the loader, not just parse. The proof is a config
/// somewhere the default resolution would never look.
#[test]
fn the_config_flag_points_at_another_file() {
    let sandbox = Sandbox::new();
    let elsewhere = sandbox.home().join("elsewhere.toml");
    std::fs::write(&elsewhere, "[repo]\nroot = \"~/somewhere-else\"\n").unwrap();

    let run = sandbox.run(&["--config", elsewhere.to_str().unwrap(), "config", "print"]);
    run.ok();
    assert!(run.stdout.contains("somewhere-else"), "{}", run.stdout);
}

/// A config in a layout scriv no longer reads is refused rather than
/// half-understood — and the error carries the replacement, because a rejection
/// with no way forward is just a broken tool. Non-zero, so a script notices.
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

/// Malformed TOML has to name the file it is in. "expected `=`" on its own
/// leaves the user hunting for which of several files scriv was reading.
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

/// The exit status is the whole reason `config check` is worth putting in a
/// setup script, so a sound setup has to come back 0 — even one with things
/// worth knowing about, since the sandbox has no fish history and no tracked
/// files and neither leaves scriv broken.
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

/// A broken setup exits non-zero and says which line was the problem, and the
/// report is still readable without colour — the statuses are shapes.
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

/// One problem, one line. Discovery treats a missing search path as a hard
/// error, so running it anyway would report the same thing twice in vaguer
/// words and inflate the failure count.
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

/// The core of `repo ls`: what is under the root, one per line, sorted, with
/// the home directory collapsed. Everything else about discovery is a variation
/// on this.
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

/// `-A` is for feeding another command, which cannot expand a `~`.
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

/// A directory in the ignore list costs its own subtree and nothing else.
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

/// With nothing configured, the answer is not an empty list — an empty list
/// reads as "you have no repositories", when the truth is "scriv was never told
/// where to look". The error says so and names the file to fix.
#[test]
fn repo_ls_without_a_root_says_what_to_do() {
    let sandbox = Sandbox::new();
    sandbox.write_config("[picker]\nheight = \"40%\"\n");

    let run = sandbox.run(&["repo", "ls"]);
    run.code(1);
    assert!(run.stderr.contains("root"), "{}", run.stderr);
    assert!(run.stderr.contains("config init"), "{}", run.stderr);
}

/// A root that is not there is a typo, and a typo has to say so rather than
/// quietly finding nothing.
#[test]
fn repo_ls_reports_a_missing_root() {
    let sandbox = Sandbox::new();
    sandbox.write_config("[repo]\nroot = \"~/not/here\"\n");

    let run = sandbox.run(&["repo", "ls"]);
    run.code(1);
    assert!(run.stderr.contains("not/here"), "{}", run.stderr);
}

// --- the known-files list ---------------------------------------------------

/// The whole lifecycle of the list through the CLI, since each step is only
/// meaningful given the one before it.
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

    let removed = sandbox.run(&["file", "remove", note.to_str().unwrap()]);
    removed.ok();
    assert!(removed.stdout.contains("Removed"), "{}", removed.stdout);
    assert!(sandbox.run(&["file", "ls"]).ok().lines().is_empty());
}

/// Adding the same path twice is a mistake worth reporting, not a silent
/// no-op — the user thinks they added something.
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

/// A tracked file can be deleted from under the list — that is the normal way
/// a list goes stale — so `--missing` and `--exists` are how you find out
/// which. The status glyphs are shapes, so a piped listing says as much as a
/// coloured one.
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

/// A relative path is resolved against where the user is standing, not against
/// wherever scriv happens to run.
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

// --- fish history -----------------------------------------------------------

/// Write a fish history file where scriv looks for one by default.
fn write_history(sandbox: &Sandbox, body: &str) {
    let path = sandbox.home().join(".local/share/fish/fish_history");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

/// Newest first, repeats collapsed, multi-line commands folded onto one row —
/// the three things that make the output one entry per line, which is what
/// makes `| grep` and `| wc -l` mean what they look like they mean.
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

/// `--status` is the sortable, cuttable form: a fixed-width local timestamp
/// then the command.
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
    // The command starts in the same column on both rows, dated or not, which
    // is what lets `cut -c18-` and `sort` work on the output.
    let column = |line: &str| line.len() - line.trim_start().len();
    assert_eq!(
        column(undated),
        "2026-07-30 13:57  ".len(),
        "an undated row did not hold the column open: {undated:?}"
    );
    assert!(undated.trim_end().ends_with("undated"), "{undated:?}");
}

/// No history file is not an empty history — it usually means the path is
/// wrong, so the error names it and the key that changes it.
#[test]
fn history_without_a_file_names_the_path_it_looked_at() {
    let sandbox = Sandbox::new();
    let run = sandbox.run(&["history", "ls"]);
    run.code(1);
    assert!(run.stderr.contains("fish_history"), "{}", run.stderr);
    assert!(run.stderr.contains("[history] file"), "{}", run.stderr);
}

/// A history file is decades of whatever was typed at a shell, and not all of
/// it is valid UTF-8. One bad byte in one entry from years ago must not cost
/// the user the other twenty thousand.
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

/// The one that earns `term::Listing`.
///
/// `scriv history ls | head -3` produces thousands of rows for a reader that
/// wants three, and `println!` answers the closed pipe with a panic — a Rust
/// stack trace where every other command-line tool simply stops. It stayed
/// hidden for as long as it did because a short listing fits in the pipe buffer
/// and the failing write never happens, so the fixture here is deliberately
/// several times the buffer's size.
#[test]
fn a_listing_ends_quietly_when_the_reader_stops_reading() {
    let sandbox = Sandbox::new();
    let mut history = String::new();
    // Comfortably past the 64 KiB pipe buffer, so the child is still writing
    // when the read end goes away.
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

/// `scriv init fish | source` is the documented setup line, so what it emits
/// has to be something fish can actually source: the helper functions, the
/// binding function the README tells people to call, and completions.
#[test]
fn init_fish_emits_functions_bindings_and_completions() {
    let sandbox = Sandbox::new();
    let run = sandbox.run(&["init", "fish"]);
    run.ok();
    for needle in [
        "function scriv-repo-cd",
        "function scriv-history-pick",
        "function fe",
        "function scriv_key_bindings",
        "complete -c scriv",
    ] {
        assert!(run.stdout.contains(needle), "missing {needle:?}");
    }
}

/// The other shells get completions only — the pick-and-`cd` helpers are
/// fish-specific — but they still have to emit something sourceable.
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

/// `init` is the one command that runs before the config is resolved, so it
/// has to work on a machine that has none — which is exactly the machine
/// someone is running it on.
#[test]
fn init_works_before_there_is_any_config() {
    let sandbox = Sandbox::new();
    assert!(!sandbox.config_path().exists());
    sandbox.run(&["init", "fish"]).ok();
}
