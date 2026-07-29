# Working on scriv

scriv scans the paths you configure for Git repositories, tracks files you
return to, opens files in your editor, and switches branches and pull
requests — all through one built-in fuzzy picker (skim).

## Before opening a pull request

- `make` — fmt check, clippy (`-D warnings`), tests, release build. All four
  must pass; CI runs the same set.
- Keep commits logical and self-contained: each one should build and test
  green on its own.

### The three docs that go stale silently

Nothing in CI compares these against the code, so a change that outdates one
looks green all the way to merge. Walk the list on every pull request and say
in the PR body which ones you touched, or why none needed it.

**1. CLI help text — `src/main.rs`.** The `about`, every doc comment on a
command or flag, and the `EXAMPLES` block are the only documentation most
users read, and clap ships them straight to the terminal. A new command or
flag is not done until it has a doc comment; a renamed or re-scoped one is not
done until the old wording is gone. `EXAMPLES` covers the common entry point
of each command group — add a line when you add a group, not when you add
every flag.

**2. README.md.** Check each of these in turn, since a change rarely touches
just one: the feature table under the demo, the `Commands` section (including
the abbreviations sentence), the key-binding table when `shell.rs` bindings
change, and the sample `config.toml` when a config key is added or renamed.
The README's command table and `scriv --help` describe the same surface and
must not disagree.

**3. `docs/demo.gif`.** If anything changed what a picker shows on screen —
row layout, colours, preview contents, the commands or flags in
`demo/demo.tape` — re-record with `make demo` and commit the GIF. CI only
plays the tape (`make demo-check`); it never commits a GIF, so a stale demo is
invisible until someone looks at the README. Recording needs `vhs`
(`brew install vhs`) and takes about 30 seconds. A change that adds a command
without altering any existing picker's output does not need a re-record — say
so in the PR rather than leaving it unmentioned.

## Shipping without being asked

Open and merge your own pull requests. That is standing authorisation for this
repository, not something to confirm each time — finishing a piece of work and
then stopping to ask permission to land it wastes the round trip that made the
work worth doing.

The gate is confidence that the change is sound, and that has a definition
here already: `make` green, each commit building and testing green on its own,
and the three docs above walked and accounted for in the PR body. Meet it and
merge. Squash-merge and delete the branch, matching the existing history.

Never commit straight to `main`. The pull request is what leaves a reviewable
record of work nobody watched happen — which matters more when there is no
reviewer standing by, not less.

Stop and say so rather than merging when the work does not clear that bar:

- a change you could not actually verify, as opposed to one you believe is fine
- a test deleted rather than replaced, or a guarantee quietly dropped
- a trade-off with a real cost that has not been named out loud in the PR body
- rewriting history that is already pushed, touching CI credentials or secrets,
  or publishing the crate

Those are worth a sentence and a pause. Everything else — ship it.

## Layout

The crate is split into an I/O-free core and an imperative shell, and new code
is expected to follow it:

| | |
| --- | --- |
| `config.rs`, `path.rs`, `files.rs` | parsing and path rules, no I/O |
| `git.rs`, `gh.rs` | ref classification, checkout resolution, JSON parsing — pure functions, plus the process helpers |
| `repo.rs` | discovery traversal rules |
| `walk.rs` | the `ignore`-crate file walk shared by `edit` and `file add` |
| `pick.rs` | the skim wrapper: rows, colours, previews |
| `cmd/*.rs` | the imperative shell — reads the environment, spawns processes, drives selection |
| `shell.rs` | the fish integration and completions that `scriv init` emits |

Decisions belong in pure functions with tests (`classify`, `resolve`,
`parse_prs`, `sanitize_file_path`); only `cmd/` and the process helpers touch
the outside world. `Ctx` resolves the environment once and is passed by
reference, so command implementations do no environment lookups of their own.

## Things that are the way they are on purpose

- **Preview panes must stay cheap.** skim runs preview commands on a background
  thread and does *not* kill non-PTY children — it only discards their output —
  so a slow command piles up copies of itself while the user scrolls. Prefer
  `Preview::Text` built from data already in hand (this is why PR descriptions
  come from the `gh pr list` call rather than `gh pr view`, which cost ~2s of
  network per row). A `Preview::Command` is acceptable only if it is local and
  bounded: tens of milliseconds, with an explicit `--max-count`/line limit.
- **git commands in previews pass `--no-optional-locks`.** A plain `git status`
  rewrites the index, so scrolling a list would take the repository's index
  lock and contend with whatever the user is running in it.
- **Preview commands are built through `pick::quote`.** A branch name or path
  containing a quote must not be able to alter the command that runs.
- **`git`, `gh` and the editor explain their own failures.** When a spawned
  child fails, return `Reported(code)` so the process exits with the child's
  status instead of printing a second, vaguer error line on top of git's.
- **The top-level commands are registries; `edit` is the exception.**
  `repo`/`file`/`branch`/`pr` are each a set scriv knows about, with `ls`/`pick`
  and a verb over that set. `edit` acts on the directory the user is standing
  in, so it is a verb at the top level rather than a fifth noun — and it has no
  `ls`. Resist filing ambient-directory work under a noun group.
- **Anything drawn inline takes a `term::ScratchRow` first.** The picker and the
  spinner both start on the row the cursor is on, which from a key binding is
  the last row of the shell's prompt — the picker draws over it, the spinner
  erases it. A one-line prompt hides that, because the shell redraws the whole
  thing; a two-line prompt is left cut in half. Take a row, draw on that, and
  step back up on the way out so the cursor is where the shell's repaint expects
  it. Never draw on the row scriv was invoked on.
- **Only `cd` needs the shell.** A child cannot change its parent's directory,
  which is why `repo pick` prints a path for a fish function to consume. Running
  an editor needs no such help: `scriv edit` spawns it directly and skim restores
  the terminal on its way out. Add a fish wrapper only for what genuinely cannot
  work from a child process.
- **Colour is dropped when stdout is not a terminal**, and `NO_COLOR` is
  honoured — `ls` output has to stay pipe-safe. Use `term::stdout_color()`.
- **Key bindings use `alt-<letter>`.** fish leaves that space entirely unbound,
  while function keys past `f3` are commonly taken by users' own tools.

## The demo

`demo/fixture.sh` builds a throwaway sandbox and `demo/demo.tape` records it
with VHS. Nothing in scriv knows it is being demoed, and it must stay that way:
the sandbox is applied from outside via `HOME`, `XDG_CONFIG_HOME`, and a stub
`gh` placed earlier on `PATH`. Never add a demo or fake-data mode to the
binary, and never let real repositories, branches, or pull requests into a
recording. `make demo-fixture` builds the same sandbox to poke at by hand.
