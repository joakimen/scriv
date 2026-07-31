# scriv

[![ci](https://github.com/joakimen/scriv/actions/workflows/ci.yml/badge.svg)](https://github.com/joakimen/scriv/actions/workflows/ci.yml)
[![release](https://img.shields.io/github/v/release/joakimen/scriv?logo=github&color=blue)](https://github.com/joakimen/scriv/releases/latest)
[![license](https://img.shields.io/github/license/joakimen/scriv?color=blue)](LICENSE)

![A candle burning above an open book](docs/art/seal.svg)

Provides fuzzy-completion for various local and remote resources.

![scriv: check out a remote branch, find and open a file, list pull requests](docs/demo.gif)

## Install

```sh
mise use -g github:joakimen/scriv    # or: cargo install --path .
```

macOS and Linux. Needs `git`; `gh` for pull requests and `repo clone`/`open`.

## Setup

```sh
scriv config init          # write ~/.config/scriv/config.toml, then set `root`
scriv init fish | source   # helpers, key bindings, completions
scriv config check         # confirm it all resolves
```

## Commands

| | |
| --- | --- |
| `scriv repo` | `ls` `pick` `open` `clone` |
| `scriv file` | `ls` `pick` `add` `remove` |
| `scriv branch` | `ls` `pick` `checkout` |
| `scriv pr` | `ls` `pick` `checkout` `open` `merge` |
| `scriv history` | `ls` `pick` |
| `scriv edit` | a file below `$PWD`, opened in `$EDITOR` |

`ls` prints the set, `pick` fuzzy-selects one line of it, and the other verbs
act. Every `pick` composes: `cd (scriv repo pick)`.

In fish, `ctrl-r` searches history, `ctrl-o` jumps to a repository, `ctrl-g`
checks out a branch and `ctrl-q` edits a file; `f1`, `f3` and `f7` open a
repository, a tracked file and a pull request.

`scriv --help` covers the flags, the generated `config.toml` the settings.

## Development

```sh
make                # fmt check, clippy, tests, release build
make demo           # re-record docs/demo.gif
make demo-fixture   # build the demo sandbox and poke at it by hand
```
