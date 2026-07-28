# scriv

[![ci](https://github.com/joakimen/scriv/actions/workflows/ci.yml/badge.svg)](https://github.com/joakimen/scriv/actions/workflows/ci.yml)

> "We found him wandering around, with a candle."

![scriv: pick a repository, check out a remote branch, list pull requests](docs/demo.gif)

[Scriv](https://kingkiller.fandom.com/wiki/Scriv) is a CLI for finding your way
around a machine. Four things, each with the same built-in fuzzy picker:

| | | Jump to |
| --- | --- | --- |
| **repos** | discover your Git repositories and jump into one | [Repositories](#repositories) |
| **files** | track the config and notes files you return to, and open one | [Files](#files) |
| **branches** | switch between local branches, check out remote ones | [Branches](#branches) |
| **pull requests** | pick a GitHub PR and check it out | [Pull requests](#pull-requests) |

Every picker previews the highlighted row, and none of them make you leave the
finder to decide.

It is self-contained: the fuzzy finder ([skim](https://github.com/skim-rs/skim))
and the file walker (the [`ignore`](https://docs.rs/ignore) crate that powers
[fd](https://github.com/sharkdp/fd)) are compiled in. No `fzf`, `fd`, or other
external tools are required. Branch commands drive `git`; pull request commands
drive [`gh`](https://cli.github.com), reusing whatever `gh auth login` already
set up — scriv stores no tokens of its own.

## Install

With [mise](https://mise.jdx.dev):

```sh
mise use -g github:joakimen/scriv
```

From source:

```sh
cargo install --path .
```

## Getting started

```sh
scriv config init          # write ~/.config/scriv/config.toml
scriv config print         # tune the search paths, then verify
scriv repo ls              # verify discovery
scriv init fish | source   # helpers, key bindings, completions
```

## Commands

| Command | Does |
| --- | --- |
| `scriv repo ls [-A]` | list discovered repositories (`-A` for absolute paths) |
| `scriv repo pick` | fuzzy-select a repository, print its absolute path |
| `scriv file ls [--status] [--missing\|--exists]` | list known files |
| `scriv file pick` | fuzzy-select a known file, print its absolute path |
| `scriv file add [path]` | add a file; omit the path to pick one from the current directory |
| `scriv file remove [path]` | remove a file; omit the path to pick interactively |
| `scriv branch ls [--local\|--remote] [--status] [--fetch]` | list local and remote branches |
| `scriv branch pick` | fuzzy-select a branch, print its name |
| `scriv branch checkout [branch]` | check out a branch; omit the name to pick one |
| `scriv pr ls [--state <state>] [--limit <n>] [--status]` | list pull requests |
| `scriv pr pick` | fuzzy-select a pull request, print its number |
| `scriv pr checkout [number]` | check out a pull request; omit the number to pick one |
| `scriv config init\|print\|path` | write, show, or locate the configuration |
| `scriv init <shell>` | print shell integration to `source` |

`scriv branch` abbreviates to `scriv br`; `checkout` to `co` (and `switch` for
branches). `ls` is also spelled `list`.

Every `pick` prints one line and nothing else, so it composes:

```fish
cd (scriv repo pick)
gh pr view (scriv pr pick)
```

## Repositories

`scriv repo` searches the paths in your config, groups the results by the label
you gave each path, and colours each group differently — so a work checkout and
a personal one are distinguishable at a glance. See
[`paths.<group>`](#pathsgroup) and [`picker.display`](#pickerdisplay).

## Files

`scriv file` keeps a list of the files you keep coming back to — dotfiles,
notes, that one YAML — so opening one is two keystrokes rather than a path you
half-remember. `file ls --status` marks which still exist; `file add` with no
argument picks from the current directory tree, honouring `.gitignore`.

## Branches

`scriv branch` lists local and remote branches together, most recently
committed to first, and colours each row by where the branch lives:

| Colour | Meaning | `--status` tag |
| ------ | ------- | -------------- |
| green  | exists here **and** on a remote | `both` |
| yellow | exists **only** here — never pushed, or its upstream is gone | `local` |
| cyan   | exists **only** on a remote | `remote` |

A remote-only branch is shown with its remote (`origin/feature`), so two remotes
carrying the same branch stay distinguishable. Checking one out creates the
matching local branch and sets its upstream — the equivalent of
`git switch --track origin/feature` — so `git push` and `git pull` work right
away. The current branch is marked with `*`.

```sh
scriv branch checkout            # pick from every branch and switch to it
scriv branch checkout --fetch    # refresh remotes (pruning deleted) first
scriv branch checkout --remote   # pick only branches that exist on a remote
scriv branch checkout main       # skip the picker
scriv branch ls --status         # marker, tag, last commit
```

Naming a branch on the command line always resolves against every branch, so
`--local`/`--remote` only narrow what the picker offers. Asking for
`origin/main` when `main` is already checked out locally switches to the local
branch rather than detaching HEAD.

`branch ls` prints bare names by default so it pipes cleanly; colour is dropped
when stdout is not a terminal, and `NO_COLOR` is honoured.

## Pull requests

`scriv pr` uses the [`gh`](https://cli.github.com) CLI, so it works wherever
`gh` is authenticated — including GitHub Enterprise — and needs no configuration
in scriv. Rows are coloured by state: green open, grey draft, magenta merged,
red closed.

```sh
scriv pr checkout                # pick an open PR and check it out
scriv pr ls --state all --limit 20
gh pr view (scriv pr pick)       # pick prints the number, so it composes
```

`pr checkout` hands off to `gh pr checkout`, which handles PRs from forks and
sets the upstream.

## Previews

Every picker previews the highlighted row, so you can tell candidates apart
without leaving the finder:

| Picker | Preview |
| ------ | ------- |
| `repo pick` | current branch, working-tree status, recent commits |
| `branch pick` / `checkout` | recent commits with **author** and relative date |
| `pr pick` / `checkout` | title, state, author, source branch, description |
| `file pick` / `add` / `remove` | file contents (via `bat` when installed, else `head`) |

A preview is extra information, never a reason for the list to feel slow, so
each one is either free or nearly so:

- Pull request previews are rendered from the listing scriv already fetched —
  no per-row network call, and no extra request.
- The rest run a local command only for the highlighted row, bounded so it stays
  in the tens of milliseconds (30 commits, 200 lines, a `head`-capped status).
- Repository and branch previews pass `--no-optional-locks`, so scrolling a list
  never takes a repository's index lock or competes with your own `git`.

Turn previews off, or move the pane, with [`picker.preview`](#pickerpreview) and
[`picker.preview_window`](#pickerpreview_window).

## Shell integration (fish)

`scriv init fish` emits helper functions, a key-binding function, and
completions. Source it from `config.fish`:

```fish
scriv init fish | source

function fish_user_key_bindings
    scriv_key_bindings
end
```

That defines four helpers and binds them:

| Key | Helper | Does |
| --- | --- | --- |
| `ctrl-o`, `alt-o` | `scriv-repo-cd` | pick a repository and `cd` into it |
| `f3` | `scriv-file-edit` | pick a known file, open it in `$EDITOR` |
| `alt-b`, `alt-g` | `scriv-branch-checkout` | pick a branch and check it out |
| `alt-p` | `scriv-pr-checkout` | pick a pull request and check it out |

The bindings are `alt-<letter>`, which fish leaves entirely unbound by default,
and each chord falls under one hand: `alt-b`/`alt-g` left, `alt-o`/`alt-p`
right. Function keys past `f3` are deliberately left alone — they are the ones
people bind to their own tools.

To use different keys, bind them *after* `scriv_key_bindings`, since the last
binding for a key wins:

```fish
function fish_user_key_bindings
    scriv_key_bindings
    bind ctrl-g "scriv-branch-checkout; commandline -f execute"
end
```

For other shells, `scriv init bash` / `zsh` / `powershell` / `elvish` emit
completions you can source or install.

## Development

```sh
make            # fmt check, clippy, tests, release build
make demo       # re-record docs/demo.gif
make demo-fixture   # build the demo sandbox and poke at it by hand
```

The demo in this README is generated, not recorded by hand. `demo/fixture.sh`
builds a throwaway sandbox — fictional repositories, branches, and pull
requests — and `demo/demo.tape` drives it with
[VHS](https://github.com/charmbracelet/vhs), so re-recording produces the same
demo rather than whatever happened to be on someone's screen.

Nothing in scriv knows it is being demoed. The sandbox is applied entirely from
outside, through seams that already existed:

- `HOME` and `XDG_CONFIG_HOME` point discovery at the sandbox, so paths render
  as `~/dev/github.com/acme/billing-api`
- commit dates are written as offsets from *now*, so relative dates read the
  same whenever it is re-recorded
- a stub `gh` earlier on `PATH` than the real one answers the pull request
  commands, so the demo needs no network and no GitHub account

Re-recording is deliberate: the render depends on the fonts installed locally,
so CI only plays the tape (`make demo-check`) to catch one that has stopped
working, and never commits a GIF of its own. Recording needs `vhs`
(`brew install vhs`).

## Configuration

Configuration lives under `$XDG_CONFIG_HOME/scriv` (default
`~/.config/scriv`):

- `config.toml` — hand-edited settings (a legacy `config.json` is still read
  when no TOML file is present)
- `files` — the known-files list, one path per line, managed by
  `scriv file add`/`remove`

Run `scriv config init` to write a starter `config.toml`.

### `config.toml`

```toml
# Directory names to skip while searching.
ignore = ["node_modules", "target"]

# Search paths are grouped by a label. repo pick shows which group a repo
# belongs to (once more than one group is configured).
[[paths.personal]]
path = "~/dev/github.com"
depth = 2

[[paths.personal]]
path = "~/bin"
depth = 0

[[paths.work]]
path = "~/work/acme"
depth = 2

[picker]
height = "50%"                # built-in finder height
display = "relative"          # repo path rendering: relative | tilde | full
preview = true                # show a preview pane for the highlighted row
preview_window = "right:50%"  # preview layout
```

### Configuration keys

#### `paths.<group>`

A named group of search paths. The group label is shown — in a colour assigned
per group — alongside each repo in `repo pick`, so you can tell at a glance
which context a repo belongs to (e.g. `personal` vs a client name). Groups
appear in the order written.

A flat `[[paths]]` list (no group) is also accepted and lands in a `default`
group.

#### `paths.<group>[].path`

Required. The root path under which to search for repos. The root path may
itself be a repo.

#### `paths.<group>[].depth`

Optional (default `0`). The search depth for the associated path. Tune this to
your project layout — it is the primary factor in discovery performance.

- `~/dev/github.com` at depth `2`: `~/dev/github.com/repo1` and
  `~/dev/github.com/dir1/repo1` are returned; `~/dev/github.com/a/b/repo1` is
  not.
- `~/bin` at depth `0`: `~/bin` is returned if it is a repo; `~/bin/repo1` is
  not.

#### `ignore`

Optional. Directory names to skip during search.
Default: `node_modules`, `vendor`, `dist`, `build`, `target`.

#### `picker.height`

Optional (default `"50%"`). Height of the built-in fuzzy finder, e.g. `"50%"`
or `"20"`. The finder is compiled in (skim); there is no `fzf` dependency.

#### `picker.display`

Optional (default `"relative"`). How `repo pick` renders each repository:

- `relative` — path relative to the search root it was found under, so the
  shared base (named by the group) is not repeated on every row:
  `personal  joakimen/scriv`.
- `tilde` — absolute path with the home directory collapsed to `~`:
  `personal  ~/dev/github.com/joakimen/scriv`.
- `full` — the full absolute path.

The selected path is always absolute regardless of this setting; only the
display changes. `repo ls` is unaffected.

#### `picker.preview`

Optional (default `true`). Whether the picker shows a preview pane for the
highlighted row — see [Previews](#previews). Set to `false` to give the list the
full width.

#### `picker.preview_window`

Optional (default `"right:50%"`). Preview pane layout, in skim's syntax:
`[up|down|left|right][:SIZE][:hidden][:[no]wrap][:[no]pty]`. For example
`"down:40%"` on narrow terminals, or `"right:50%:hidden"` to keep the pane
collapsed until toggled.
