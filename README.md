# scriv

[![ci](https://github.com/joakimen/scriv/actions/workflows/ci.yml/badge.svg)](https://github.com/joakimen/scriv/actions/workflows/ci.yml)
[![release](https://img.shields.io/github/v/release/joakimen/scriv?logo=github&color=blue)](https://github.com/joakimen/scriv/releases/latest)
[![license](https://img.shields.io/github/license/joakimen/scriv?color=blue)](LICENSE)

![A candle burning above an open book](docs/art/seal.svg)

One fuzzy picker for your Git repositories, files, branches, pull requests and
shell history.

![scriv: check out a remote branch, find and open a file, list pull requests](docs/demo.gif)

| | |
| --- | --- |
| **repos** | pick one and `cd` into it, open it on GitHub, or clone new ones |
| **files** | keep a list of files you return to, and open one |
| **editing** | fuzzy-find a file where you are standing and open it in `$EDITOR` |
| **branches** | switch to a local branch, or check out a remote one |
| **pull requests** | see what is green, then check one out, open it, or merge it |
| **shell history** | fuzzy-search what you have run before, on `ctrl-r` and `up` |

One binary and nothing else: the fuzzy finder
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

macOS and Linux, with `git`. `gh` is needed only for pull requests and
`repo clone`/`open`; the fish integration only for the key bindings.

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
  when you are not. `--pick` asks even from inside one.
- **`scriv pr open` / `scriv pr merge`** — put a pull request in the browser, or
  merge it. Both fuzzy-pick when given no number; `merge` takes `--squash`,
  `--auto`, `-d` and the rest of `gh`'s vocabulary.
- **`scriv file add` / `remove`** — maintain the tracked list.

`scriv edit` is a verb rather than a noun: it searches the directory you are
standing in — not a list scriv keeps — and opens what you choose in `$VISUAL`,
then `$EDITOR`. The walk honours `.gitignore`, `.ignore` and `.fdignore` and
streams into the picker as it goes, so a huge tree is typeable immediately.

```fish
scriv edit              # pick a file below $PWD, open it in your editor
scriv edit --tracked    # pick from your tracked files instead
scriv edit src/main.rs  # skip the picker
```

`scriv config` and `scriv init` handle setup. `branch` abbreviates to `br`,
`history` to `hist`, `edit` to `e`, `checkout` to `co`, and `ls` to `list`. Any
command takes `--help` for its flags.

Every list bar history previews the highlighted row: commits and working-tree
state for repositories and branches, description and failing checks for pull
requests, contents for files. Pull request listings carry their CI, so what is
green is visible before you pick anything:

```
#128  ✓    Add a token bucket per API key  @ada
#127  ✗    Round partial usage up  @grace
#126  ⧗ ⊘  Cache quotas in redis  @ada
```

`✓` passed, `✗` failed, `⧗` still running, `⊘` conflicts with the base branch —
shapes rather than colours alone, so a piped or `NO_COLOR` listing says as much.
Branch and pull request lists go stale while you read them; `ctrl-r` refetches
without closing the picker.

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
| `f1` | open this repository on GitHub, or pick one |
| `f3` | pick a tracked file and open it in `$EDITOR` |
| `f7` | pick a pull request and check it out |

`ctrl-r` and `up` take over fish's history keys, searching the same history
fuzzily rather than by prefix and starting from whatever you have already typed.
`up` still moves the cursor wherever a picker would be wrong — in the completion
pager, and past the first line of a multi-line command. To use your own keys,
bind them after `scriv_key_bindings`; the last binding for a key wins.

`fe` — find, fuzzy-pick, edit — is a short alias for `scriv edit`, arguments and
all, and the only unprefixed name scriv defines. For other shells,
`scriv init bash`/`zsh`/`powershell`/`elvish` emit completions.

## Configuration

`scriv config init` writes `~/.config/scriv/config.toml`, where settings are
grouped by the command that reads them:

```toml
[repo]
# One root, laid out as <owner>/<repo> — the same shape as GitHub itself, which
# is how `repo clone` knows where a repository belongs without being told.
root = "~/dev/github.com"
extra = ["~/bin"]                   # repositories outside the root
ignore = ["node_modules", "target"] # directory names to skip
display = "relative"                # relative | tilde | full

# Labels name owners, one label to many, so everything you touch for work
# colours as one group however many orgs it spans.
labels = { personal = ["joakimen"], work = ["capralifecycle", "nsbno"] }

[history]
# fish's history file. Defaults to $XDG_DATA_HOME/fish/fish_history. Worth
# setting only for a named session — `set -U fish_history work` reads
# `work_history`, and fish does not export that variable for scriv to find.
file = "~/.local/share/fish/work_history"

[picker]
height = "50%"                # finder height
preview = true                # preview pane for the highlighted row
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

The demo is generated, not captured — `demo/fixture.sh` builds a throwaway
sandbox of fictional repositories, branches and pull requests, and recording it
needs [VHS](https://github.com/charmbracelet/vhs).
