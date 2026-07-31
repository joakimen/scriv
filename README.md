# scriv

[![ci](https://github.com/joakimen/scriv/actions/workflows/ci.yml/badge.svg)](https://github.com/joakimen/scriv/actions/workflows/ci.yml)
[![release](https://img.shields.io/github/v/release/joakimen/scriv?logo=github&color=blue)](https://github.com/joakimen/scriv/releases/latest)
[![license](https://img.shields.io/github/license/joakimen/scriv?color=blue)](LICENSE)

![A candle burning above an open book](docs/art/seal.svg)

Fuzzy navigation and management of common local and remote resources.

![scriv: check out a remote branch, find and open a file, list pull requests](docs/demo.gif)

| | |
| --- | --- |
| **repos** | pick one of your repositories and `cd` into it, open it on GitHub, or clone new ones |
| **files** | keep a list of files you return to, and open one |
| **editing** | fuzzy-find a file where you are standing and open it in `$EDITOR` |
| **branches** | switch to a local branch, or check out a remote one |
| **pull requests** | see what is green, then check one out, open it, or merge it |
| **shell history** | fuzzy-search what you have run before, on `ctrl-r` and `up` |

It is one binary and nothing else: the fuzzy finder
([skim](https://github.com/skim-rs/skim)) and the file walker (the
[`ignore`](https://docs.rs/ignore) crate behind
[fd](https://github.com/sharkdp/fd)) are compiled in. Branches come from `git`,
pull requests from [`gh`](https://cli.github.com) — reusing the login
`gh auth login` already set up, so scriv stores no tokens of its own.

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
scriv config print         # set your root, then check it
scriv repo ls              # see what scriv finds under it
scriv init fish | source   # helpers, key bindings, completions
```

## Commands

Five nouns, the same few verbs:

| | `ls` lists | `pick` prints | `checkout` |
| --- | --- | --- | --- |
| `scriv repo` | your repositories | an absolute path | — |
| `scriv file` | your tracked files | an absolute path | — |
| `scriv branch` | local and remote branches | a branch name | switches, tracking the remote |
| `scriv pr` | pull requests | a PR number | hands off to `gh pr checkout` |
| `scriv history` | commands you have run | a past command | — |

Some verbs belong to one noun only:

- **`scriv repo clone`** — pick a GitHub owner, then one or more of their
  repositories; `tab` selects several and they clone at once. Everything lands
  at `<root>/<owner>/<repo>`, so a clone is in `repo pick` the moment it
  finishes. `scriv repo clone owner/repo` skips both pickers.
- **`scriv repo open`** — opens the repository you are standing in, or picks one
  when you are not standing in any. `--pick` asks even from inside one.
- **`scriv pr open` / `scriv pr merge`** — put a pull request in the browser, or
  merge it. Both fuzzy-pick when given no number; `merge` takes `--squash`,
  `--auto`, `-d` and the rest of `gh`'s vocabulary.
- **`scriv file add` / `remove`** — maintain the tracked list.

`scriv config` and `scriv init` handle setup. `branch` abbreviates to `br`,
`history` to `hist`, `edit` to `e`, `checkout` to `co`, and `ls` to `list`. Any command takes
`--help` for its flags.

`scriv history` searches what you have already run, reading fish's own history
file — newest first, repeats of a command collapsed onto one row. `pick` prints
the command rather than running it; the fish integration puts it back on the
command line, to be read before you press enter.

```fish
scriv history pick          # fuzzy-pick a past command, print it
scriv history ls --status   # every command, dated with when you last ran it
```

Every row is dated with when you last ran that command, in local time:

```
2026-07-30 13:58  gh pr merge --squash --auto
2026-07-30 13:57  git log --oneline -3
2026-07-29 09:11  cargo test --lib
```

The date is shown but never searched — it is digits at the front of every row,
and matching it would put four thousand timestamps ahead of the command you were
reaching for. `--status` prints the same column, fixed-width and sortable, so
`sort` and `cut` work on it.

fish records *when* a command ran and nothing else — there is no duration or
exit status in its history file, so scriv cannot show what it was never told.

A command spanning several lines is folded onto its one row with a `⏎` where
each break was, so it cannot be mistaken for a command you never ran; selecting
it yields the real multi-line text. The history picker has no preview pane —
the pane would only repeat the row beside it, and the command lands back on the
command line to be read before it runs. This one is fish-only: it is fish's
history format, and only a shell can write to its own command line.

`scriv edit` is the one verb rather than a noun: it searches the directory you
are standing in — not a list scriv keeps — and opens what you choose in
`$VISUAL`, then `$EDITOR`.

```fish
scriv edit              # pick a file below $PWD, open it in your editor
scriv edit --tracked    # pick from your tracked files instead
scriv edit src/main.rs  # skip the picker
```

The walk honours `.gitignore`, `.ignore` and `.fdignore`, and streams into the
picker as it goes, so a huge tree is typeable immediately rather than once the
last file is found. `tab` selects several and they open together.

Every list bar history previews the highlighted row, so you can tell candidates
apart without leaving the picker: commits and working-tree state for
repositories and branches, description and failing checks for pull requests,
contents for files.

Pull request listings carry their CI, so what is green is visible before you
pick anything:

```
#128  ✓    Add a token bucket per API key  @ada
#127  ✗    Round partial usage up  @grace
#126  ⧗ ⊘  Cache quotas in redis  @ada
```

`✓` passed, `✗` failed, `⧗` still running, `⊘` conflicts with the base branch —
shapes rather than colours alone, so a piped or `NO_COLOR` listing says exactly
as much. Branch and pull request lists go stale while you read them, so `ctrl-r`
refetches and swaps the rows in underneath your query without closing the
picker.

Every `pick` prints one line and nothing else, so it composes:

```fish
cd (scriv repo pick)
gh pr view (scriv pr pick)
```

## Shell integration (fish)

```fish
scriv init fish | source

function fish_user_key_bindings
    scriv_key_bindings
end
```

| Key | Does |
| --- | --- |
| `ctrl-r` | search shell history, onto the command line |
| `up` | the same, on the first line of a prompt |
| `ctrl-o` | pick a repository and `cd` into it |
| `ctrl-g` | pick a branch and check it out |
| `ctrl-q` | pick a file below `$PWD` and open it in `$EDITOR` |
| `f1` | open this repository on GitHub, or pick one when you are not in one |
| `f3` | pick a tracked file and open it in `$EDITOR` |
| `f7` | pick a pull request and check it out |

`ctrl-r` and `up` take over fish's history keys, searching the same history
fuzzily instead of by prefix and starting from whatever you have already typed.
`up` hands back to fish wherever a picker would be wrong — in the completion
pager, and on any line of a multi-line command but the first, where it still
moves the cursor — and `ctrl-p`/`ctrl-n` are untouched, so stepping through
history one entry at a time is still there. Of the rest only `ctrl-g` displaces
anything fish had bound (`cancel`, which `escape` and `ctrl-c` also do); the
function keys and the others were free. To use your own keys, bind them after
`scriv_key_bindings` — the last binding for a key wins.

The integration also defines `fe` — find, fuzzy-pick, edit — as a short alias
for `scriv edit`, arguments and all. That is the only unprefixed name scriv
defines. For other shells, `scriv init bash`/`zsh`/`powershell`/`elvish` emit
completions.

## Configuration

`scriv config init` writes `~/.config/scriv/config.toml`, where settings are
grouped by the command that reads them:

```toml
# `scriv repo`: where your repositories are, and how they are labelled.
[repo]

# One root, laid out as <owner>/<repo> — the same shape as GitHub itself, which
# is how `repo clone` knows where a repository belongs without being told.
root = "~/dev/github.com"

# Repositories outside the root, listed one at a time.
extra = ["~/bin"]

# Directory names to skip while searching.
ignore = ["node_modules", "target"]

# Repo path rendering: relative | tilde | full
display = "relative"

# Labels name owners, one label to many, so everything you touch for work
# colours as one group however many orgs it spans.
labels = { personal = ["joakimen"], work = ["capralifecycle", "nsbno"] }

# `scriv history`: which shell history to search.
[history]

# fish's history file. Defaults to $XDG_DATA_HOME/fish/fish_history, falling
# back to ~/.local/share/fish/fish_history. Only worth setting if you have named
# your session — `set -U fish_history work` reads `work_history` instead — since
# fish does not export that variable for scriv to find.
file = "~/.local/share/fish/work_history"

# The built-in fuzzy picker, shared by every command that opens one.
[picker]
height = "50%"                # finder height
preview = true                # show a preview pane for the highlighted row
preview_window = "right:50%"  # preview layout
```

The tracked-files list sits beside it in `~/.config/scriv/files`, one path per
line, managed by `scriv file add`/`remove`. There is no key for the editor:
`$VISUAL` and `$EDITOR` are where every other terminal tool reads it. A config
still using an older layout is refused with the replacement written out for you.

## Development

```sh
make                # fmt check, clippy, tests, release build
make demo           # re-record docs/demo.gif
make demo-fixture   # build the demo sandbox and poke at it by hand
```

The demo is generated, not captured: `demo/fixture.sh` builds a throwaway
sandbox of fictional repositories, branches and pull requests, applied entirely
from outside through `HOME`, `XDG_CONFIG_HOME` and a stub `gh` earlier on
`PATH`. Nothing in scriv knows it is being demoed. Recording needs
[VHS](https://github.com/charmbracelet/vhs).
