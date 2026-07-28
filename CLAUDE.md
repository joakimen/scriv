# Working on scriv

scriv scans the paths you configure for Git repositories, tracks files you
return to, and switches branches and pull requests — all through one built-in
fuzzy picker (skim).

## Before opening a pull request

- `make` — fmt check, clippy (`-D warnings`), tests, release build. All four
  must pass; CI runs the same set.
- **If anything changed what a picker shows on screen — row layout, colours,
  preview contents, the commands or flags in `demo/demo.tape` — re-record the
  demo with `make demo` and commit the resulting `docs/demo.gif`.** CI only
  plays the tape (`make demo-check`); it never commits a GIF, so a stale demo
  is invisible until someone looks at the README. Recording needs `vhs`
  (`brew install vhs`) and takes about 30 seconds.
- Keep commits logical and self-contained: each one should build and test
  green on its own.

## Layout

The crate is split into an I/O-free core and an imperative shell, and new code
is expected to follow it:

| | |
| --- | --- |
| `config.rs`, `path.rs`, `files.rs` | parsing and path rules, no I/O |
| `git.rs`, `gh.rs` | ref classification, checkout resolution, JSON parsing — pure functions, plus the process helpers |
| `repo.rs` | discovery traversal rules |
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
- **`git` and `gh` explain their own failures.** When a spawned child fails,
  return `Reported(code)` so the process exits with the child's status instead
  of printing a second, vaguer error line on top of git's.
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
