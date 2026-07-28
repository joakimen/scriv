# scriv

[![ci](https://github.com/joakimen/scriv/actions/workflows/ci.yml/badge.svg)](https://github.com/joakimen/scriv/actions/workflows/ci.yml)

![A candle burning above an open book](docs/art/seal.svg)

[Scriv](https://kingkiller.fandom.com/wiki/Scriv) puts the things you work with
behind one fuzzy picker: your repositories, the files you keep returning to, git
branches, and GitHub pull requests. You pick from a list rather than recalling a
path or a branch name and typing it out.

You tell scriv where your repositories live; branches and pull requests it reads
from `git` and `gh`. Every list previews the highlighted row, so you can tell
candidates apart without leaving the picker.

![scriv: pick a repository, check out a remote branch, list pull requests](docs/demo.gif)

| | |
| --- | --- |
| **repos** | pick one of your repositories and `cd` into it |
| **files** | keep a list of files you return to, and open one |
| **branches** | switch to a local branch, or check out a remote one |
| **pull requests** | check out a GitHub pull request |

It is self-contained: the fuzzy finder ([skim](https://github.com/skim-rs/skim))
and the file walker (the [`ignore`](https://docs.rs/ignore) crate behind
[fd](https://github.com/sharkdp/fd)) are compiled in — no `fzf`, no `fd`, no
external tools. Branch commands drive `git`; pull request commands drive
[`gh`](https://cli.github.com), reusing whatever `gh auth login` already set up,
so scriv stores no tokens of its own.

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
scriv config print         # set your search paths, then check them
scriv repo ls              # see what scriv finds under them
scriv init fish | source   # helpers, key bindings, completions
```

## Commands

Four nouns, the same few verbs:

| | `ls` lists | `pick` prints | `checkout` |
| --- | --- | --- | --- |
| `scriv repo` | your repositories | an absolute path | — |
| `scriv file` | your tracked files | an absolute path | — |
| `scriv branch` | local and remote branches | a branch name | switches, tracking the remote |
| `scriv pr` | pull requests | a PR number | hands off to `gh pr checkout` |

`scriv file add`/`remove` maintain the tracked list, and `scriv config` /
`scriv init` handle setup. `branch` abbreviates to `br`, `checkout` to `co`,
`ls` to `list`. Any command takes `--help` for its flags.

Every `pick` prints one line and nothing else, so it composes:

```fish
cd (scriv repo pick)
gh pr view (scriv pr pick)
```

Previews show commits and working-tree status for repositories and branches,
title and description for pull requests, and contents for files. They come from
data already fetched, or from local commands kept to tens of milliseconds, so
scrolling a long list never stalls.

## Shell integration (fish)

```fish
scriv init fish | source

function fish_user_key_bindings
    scriv_key_bindings
end
```

| Key | Does |
| --- | --- |
| `ctrl-o`, `alt-o` | pick a repository and `cd` into it |
| `f3` | pick a tracked file and open it in `$EDITOR` |
| `alt-b`, `alt-g` | pick a branch and check it out |
| `alt-p` | pick a pull request and check it out |

To use your own keys, bind them after `scriv_key_bindings` — the last binding
for a key wins. For other shells, `scriv init bash`/`zsh`/`powershell`/`elvish`
emit completions.

## Configuration

`scriv config init` writes `~/.config/scriv/config.toml`:

```toml
# Directory names to skip while searching.
ignore = ["node_modules", "target"]

# Where to look for repositories. Paths are grouped by a label, and repo pick
# shows the group a repo belongs to in a colour of its own, so work and
# personal checkouts stay apart.
[[paths.personal]]
path = "~/dev/github.com"
depth = 2                     # how far below the path to look

[[paths.work]]
path = "~/work/acme"
depth = 2

[picker]
height = "50%"                # built-in finder height
display = "relative"          # repo path rendering: relative | tilde | full
preview = true                # show a preview pane for the highlighted row
preview_window = "right:50%"  # preview layout
```

`depth` is the main lever on how long a scan takes; keep it as low as your
layout allows. The tracked-files list sits beside the config in
`~/.config/scriv/files`, one path per line, managed by `scriv file add`/`remove`.

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
