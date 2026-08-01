# Working on scriv

scriv scans the paths you configure for Git repositories, tracks files you
return to, opens files in your editor, switches branches and pull requests, and
searches your shell history — all through one built-in fuzzy selector (skim).

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
done until the old wording is gone.

`EXAMPLES` is three lines and stays three lines. It once carried one per
command group, which was thirteen lines of identical shape sitting directly
below the list of commands that already named every one of them — a wall to
scroll past rather than something read. What it shows now is one of each
*kind*: what to run first, how a `sel` composes into the shell, and a verb
acting on its own. A new command group does not earn a line; it earns one only
by being a kind of thing none of the three already demonstrates.

Aliases are declared `visible_alias`, never `alias`. clap hides the latter, and
a name the binary accepts but the help does not mention is exactly the drift
this section exists to prevent.

**2. README.md.** It is deliberately short — a page, not a manual — and it stays
that way. Three things in it can go stale: the command table, which must list
the same subcommands `scriv <group> --help` does; the fish key-binding sentence,
when `scriv_key_bindings` changes; and the `Install` line naming the platforms
and the external tools each command group needs.

Resist adding to it. The README's job is to get someone from nothing to a
working setup and then hand off — to `scriv --help` for flags, to the generated
`config.toml` for settings, to `CLAUDE.md` for why anything is the way it is.
Every one of those is closer to the code and cannot drift from it the way a
paragraph here can. A new flag does not belong in the README; a new *command
group* does, as one table row.

**3. `docs/demo.gif`.** If anything changed what a selector shows on screen —
row layout, colours, preview contents, the commands or flags in
`demo/demo.tape` — re-record with `make demo` and commit the GIF. CI only
plays the tape (`make demo-check`); it never commits a GIF, so a stale demo is
invisible until someone looks at the README. Recording needs `vhs`
(`brew install vhs`) and takes about 30 seconds. A change that adds a command
without altering any existing selector's output does not need a re-record — say
so in the PR rather than leaving it unmentioned.

## Shipping without being asked

Open and merge your own pull requests. That is standing authorisation for this
repository, not something to confirm each time — finishing a piece of work and
then stopping to ask permission to land it wastes the round trip that made the
work worth doing.

The gate is confidence that the change is sound, and that has a definition
here already: `make` green, each commit building and testing green on its own,
and the three docs above walked and accounted for in the PR body. Meet it and
merge.

Merging means handing the PR to GitHub rather than sitting and watching CI:

```
gh pr merge --squash --auto
```

`--auto` lands the PR itself once the checks pass, squash-merging to match the
existing history, with no poll loop in between. It is load-bearing that `build`
and `demo` are *required* status checks on `main` (the `default` ruleset):
auto-merge only queues behind checks that are required, so if that rule is ever
removed, `--auto` stops waiting and merges on the spot. They are also `strict`,
which is what stops a pull request merging against a `main` its CI never saw.

Three things auto-merge does not do for you:

- **The local branch survives.** `gh` exits the moment auto-merge is armed, so
  `--delete-branch` never gets to run — and the squash rewrites the commit, so
  even afterwards `git branch -d` calls the branch unmerged. Clean up with
  `git checkout main && git pull && git branch -D <branch>` once it lands. The
  remote branch is handled by the repo's `delete_branch_on_merge`.
- **A failure tells nobody.** A red check leaves the PR sitting open
  indefinitely. Say in the reply that auto-merge is armed, and check back —
  with a backgrounded wait on `gh pr view <n> --json state`, or on the next
  turn — so a failure gets fixed rather than silently parked.
- **It does not rebase.** The ruleset requires branches to be up to date, so a
  pull request whose base has moved *blocks* rather than merging, and stays
  blocked until someone rebases it. See below.

### Parallel pull requests land one at a time

Required checks are `strict`: a pull request cannot merge while `main` has
commits it does not have. That is not tidiness — it is the only thing that makes
CI's answer mean anything. Without it, `build` passing says two commits were
each fine on their own, which is not the question; a pull request that removed a
function and one written in parallel that still called it were both green and
the merge did not compile.

So several pull requests in flight merge in sequence, not at once:

1. Arm auto-merge on the one that should land first.
2. Wait for it (`gh pr view <n> --json state`).
3. `git checkout main && git pull`, then `git rebase main` in each remaining
   worktree, resolve, `make`, force-push with `--force-with-lease`, and arm the
   next.

Order them so the one touching the most files goes first — everything after it
rebases onto a base that is already settled. When two changes couple across
files git cannot conflict on, that is what the rebase is for: pin the coupling
with a test at that point, rather than leaving it for whoever next touches
either side.

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

## Feature work happens in a worktree

Anything larger than a one-line fix gets its own `git worktree`, not a branch
switched in place. The point is not tidiness: a worktree is a separate directory
with its own checkout, so two pieces of work can be built and tested at the same
time without one's `target/` or half-finished edits leaking into the other. Work
in place and the repository can only hold one idea at a time, which is what
rules out running them in parallel at all.

Add it under `.claude/worktrees/`, and remove it once its pull request has
merged — the branch is gone by then, and a worktree left behind is a stale
checkout that `git worktree list` will keep offering:

```
git worktree add .claude/worktrees/<name> -b <name>
git worktree remove .claude/worktrees/<name>
```

Trivial fixes can stay in the main checkout. Say which you chose.

## Layout

The crate is split into an I/O-free core and an imperative shell, and new code
is expected to follow it:

| | |
| --- | --- |
| `config.rs`, `path.rs`, `files.rs` | parsing and path rules, no I/O |
| `git.rs`, `gh.rs` | ref classification, checkout resolution, JSON parsing — pure functions, plus the process helpers |
| `history.rs` | fish history file location, parsing and row rendering, no I/O |
| `repo.rs` | discovery traversal rules |
| `walk.rs` | the `ignore`-crate file walk shared by `edit` and `file add` |
| `select.rs` | the skim wrapper: rows, colours, previews |
| `cmd/*.rs` | the imperative shell — reads the environment, spawns processes, drives selection |
| `shell.rs` | the fish integration and completions that `scriv init` emits |
| `tests/cli.rs` | end-to-end runs of the built binary — the wiring the unit tests cannot see |

Decisions belong in pure functions with tests (`classify`, `resolve`,
`parse_prs`, `sanitize_file_path`); only `cmd/` and the process helpers touch
the outside world. `Ctx` resolves the environment once and is passed by
reference, so command implementations do no environment lookups of their own.

`tests/cli.rs` covers what those unit tests cannot: that a flag reaches the
function it names, that an error leaves the right exit status behind, and that
stdout carries what a shell is expected to read. Every run there points `HOME`,
`XDG_CONFIG_HOME`, `XDG_DATA_HOME` and `PWD` at a temporary directory and wipes
the rest of the environment — a test that passed only on a machine with a
particular `~/.config/scriv` would be worse than no test. A new command or flag
that a script can call gets a case there; the interactive paths do not, since a
test that allocated a pty would be testing skim.

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
- **Preview commands are built through `select::quote`.** A branch name or path
  containing a quote must not be able to alter the command that runs.
- **`git`, `gh` and the editor explain their own failures.** When a spawned
  child fails, return `Reported(code)` so the process exits with the child's
  status instead of printing a second, vaguer error line on top of git's.
- **The top-level commands are registries; `edit` is the exception.**
  `repo`/`file`/`branch`/`pr`/`history` are each a set scriv knows about, with
  `ls`/`sel` and a verb over that set. `edit` acts on the directory the user is
  standing in, so it is a verb at the top level rather than one more noun — and
  it has no `ls`. Resist filing ambient-directory work under a noun group.
  `repo open` is the one place a registry verb reads the ambient directory, and
  only to skip a question it can already answer: in a repository it opens that
  one, `--select` asks anyway, and outside one it is the ordinary selector. The set
  it selects over is unchanged.
- **A verb is abbreviated in its own name, not in an alias beside it.** `ls`,
  `sel` and `rm` are already as short as they are going to get, so none of them
  carries a long form to alias back to — a second spelling is another row in the
  help text, another completion, and another thing to remember, for a name that
  was not long enough to be worth shortening. (`list`, `co` and `switch` predate
  the rule and are kept for the fingers already trained on them; they are not a
  pattern to extend.) Command *groups* are the exception, because a group name
  is a noun and `repository` does not abbreviate to itself: each carries one
  letter — `r`, `f`, `b`, `e`, `h`, `c`, and `pc` for `proc`, since `p` would
  collide with `pr`, which is short already.
- **`$PWD` is preferred over `getcwd`, but only when it is still true.** A shell
  sets `$PWD` to the path the user walked, symlinks intact, which is what they
  expect to see back; `getcwd` resolves those away. But nothing keeps the
  variable current — a `chdir` in a parent process leaves it behind, and it can
  simply be set wrong — and a stale one silently records a path for a directory
  the user was never standing in. `Ctx::load` takes it only when it resolves to
  the same directory scriv is actually in. Anything else reading the environment
  for a location it will then write down owes the same check.
- **A new dependency on the outside world gets a `config check` row.** Every
  path scriv reads, tool it spawns, or environment variable it needs is
  something a user can have wrong, and `scriv config check` is the one place
  that says so without them having to trip over it one command at a time. Add
  the row in `cmd/config.rs`, and choose the status honestly: `Fail` only when
  scriv is genuinely broken without it, since the exit status is what makes the
  command usable in a setup script. A check that repeats what an earlier one
  already said is not a second problem — skip it, as `discovery_check` does.
- **Anything drawn inline takes a `term::ScratchRow` first.** The selector and the
  spinner both start on the row the cursor is on, which from a key binding is
  the last row of the shell's prompt — the selector draws over it, the spinner
  erases it. A one-line prompt hides that, because the shell redraws the whole
  thing; a two-line prompt is left cut in half. Take a row, draw on that, and
  step back up on the way out so the cursor is where the shell's repaint expects
  it. Never draw on the row scriv was invoked on.
- **The selector watches for its terminal disappearing.** skim's input loop does
  not stop when its event stream ends: on a pty whose other end has closed,
  `crossterm` reports end-of-stream immediately and forever and skim's `select!`
  treats that as nothing to do, so the process pins a core until something kills
  it — one was found here having done so for over a day. Normally `SIGHUP` ends
  scriv first; `term::watch_for_hangup` is for when it does not, and it probes
  with a *zero-length write* so it can neither disturb what skim has drawn nor
  take a keystroke skim was going to act on. On macOS that is the only probe
  that works: `poll` reports no hangup and `tcgetattr`, `TIOCGWINSZ` and
  `tcgetpgrp` all keep answering normally on a slave whose master is gone. Take
  the guard out only once skim's loop terminates on its own.
- **Only `cd` needs the shell.** A child cannot change its parent's directory,
  which is why `repo sel` prints a path for a fish function to consume. Running
  an editor needs no such help: `scriv edit` spawns it directly and skim restores
  the terminal on its way out. Add a fish wrapper only for what genuinely cannot
  work from a child process.
- **Colour is decided once, on `Ctx`.** `--color auto|always|never` wins, then
  `SCRIV_NO_COLOR`, then whether stdout is a terminal — so `ls` output stays
  pipe-safe by default and `--color always` can still feed `less -R`. The
  variable is scriv's own: the cross-tool `NO_COLOR` convention is deliberately
  not read, because it is one switch for every tool at once and this is a switch
  for this one, and sharing the name would leave no way to keep scriv coloured
  in an environment that had turned everything else plain. Printing code reads
  `ctx.color()` and never asks the terminal itself; one run must not colour one
  command's output and not another's. The selector is out of scope: it is a
  terminal UI that only ever draws on a terminal.
- **Key bindings prefer `ctrl-<letter>`, and never use `alt-`.** ctrl is the
  modifier people put under a pinky, so it is where a selector worth a keystroke
  belongs. fish leaves only `ctrl-o` and `ctrl-q` unbound, so anything further
  has to displace a preset binding deliberately and say which one in the comment
  above `scriv_key_bindings` — and some presets are not worth displacing:
  `ctrl-p`/`ctrl-n` are `up-line`/`down-line`, the last way left to walk history
  one entry at a time now that `ctrl-r` and `up` open a selector instead. Taking a
  key people press all day is only justified when the selector does that key's own
  job better, on the same data, and hands control back everywhere it would not —
  which is what `scriv-history-up` is: it checks for the completion pager and
  for a cursor past line 1, and calls fish's `up-line` there. Hand back with
  `commandline -f <input function>`; a plain shell function of fish's such as
  `up-or-search` is rejected, and from inside a binding nobody sees the error.
  alt is not the escape hatch it looks like: fish binds
  most of `alt-<letter>` (`alt-b`, `alt-e`, `alt-o`, `alt-p` among them).
  `ctrl-i`/`ctrl-j`/`ctrl-m` are tab/newline/enter and are never bindable. Once
  the worthwhile ctrl chords are gone, function keys are the fallback: fish binds
  none of `f1`-`f12`, so they displace nothing, but `f4` and `f5` are where
  users' own tools cluster and are skipped.

## The demo

`demo/fixture.sh` builds a throwaway sandbox and `demo/demo.tape` records it
with VHS. Nothing in scriv knows it is being demoed, and it must stay that way:
the sandbox is applied from outside via `HOME`, `XDG_CONFIG_HOME`, and a stub
`gh` placed earlier on `PATH`. Never add a demo or fake-data mode to the
binary, and never let real repositories, branches, or pull requests into a
recording. `make demo-fixture` builds the same sandbox to poke at by hand.
