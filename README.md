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
| `scriv repo` | `ls` `sel` `open` `clone` |
| `scriv file` | `ls` `sel` `add` `rm` `prune` |
| `scriv branch` | `ls` `sel` `checkout` |
| `scriv pr` | `ls` `sel` `checkout` `open` `merge` |
| `scriv proc` | `ls` `sel` `kill` |
| `scriv history` | `ls` `sel` |
| `scriv edit` | `file` `dir` — found below `$PWD`, opened in `$EDITOR` |

`ls` prints the set, `sel` fuzzy-selects one line of it, and the other verbs
act. Every `sel` composes: `cd (scriv repo sel)`. Each group takes a one-letter
abbreviation — `r`, `f`, `b`, `e`, `h`, `c`, and `pc` for `proc`.

In fish, `ctrl-r` searches history, `ctrl-o` jumps to a repository and `ctrl-g`
checks out a branch; `f1`, `f3` and `f7` open a repository, a tracked file and a
pull request.

`scriv --help` covers the flags, the generated `config.toml` the settings.

## Development

```sh
make                # fmt check, clippy, tests, release build
make hooks          # install the git hooks in prek.toml
make release         # bump the version and open the release PR
make release-publish # after the PR merges: hand the version to dist
make release-dry-run # build every target, release nothing
make demo            # re-record docs/demo.gif
make demo-fixture    # build the demo sandbox and poke at it by hand
```

## Releasing

Releasing is two commands. The version bump lands through a PR because the
`main` ruleset requires the `build` and `demo` checks on the branch tip, so it
cannot be pushed straight to `main`.

```sh
make release          # bump the version, open the PR, arm squash auto-merge
# ...wait for the PR to merge...
make release-publish  # hand the version on main to dist
```

`make release` prompts for a level (patch/minor/major); cargo-release shows the
resolved version and asks before it changes anything.

Everything after that belongs to [dist](https://axodotdev.github.io/cargo-dist),
configured in `dist-workspace.toml`: it refuses a version no package carries,
builds macOS and Linux on x86_64 and arm64, and creates the tag and the release
together once four tarballs with checksums and build provenance exist. There is
no tag to push and none to get wrong. `.github/workflows/release.yml` is
generated — change `dist-workspace.toml` and run `dist init`, never the workflow.

`make release-dry-run` runs the same builds and releases nothing.
