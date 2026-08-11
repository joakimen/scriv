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
make release         # dispatch the version bump; opens a PR
make release-publish # after that PR merges: dispatch the release
make release-dry-run # build every target, release nothing
make demo            # re-record docs/demo.gif
make demo-fixture    # build the demo sandbox and poke at it by hand
```

## Releasing

Releasing runs entirely in GitHub Actions, in two dispatches with a pull request
between them. Nothing is built, versioned or signed on a maintainer's machine.

```sh
make release LEVEL=minor  # dispatch release-prepare; LEVEL is patch by default
# ...review and merge the pull request it opens...
make release-publish      # dispatch release for the version now on main
```

Both commands are `gh workflow run`, so the Actions tab does just as well.

`release-prepare` bumps the version with cargo-release and opens the pull
request. That pull request is the review point, and it exists because the `main`
ruleset requires the `build` and `demo` checks on the branch tip.

`release` belongs to [dist](https://axodotdev.github.io/cargo-dist), configured
in `dist-workspace.toml`: it refuses a version no package carries, builds macOS
and Linux on x86_64 and arm64, and creates the tag and the release together once
four tarballs with checksums and build provenance exist. There is no tag to push
and none to get wrong. `.github/workflows/release.yml` is generated — change
`dist-workspace.toml` and run `dist init`, never the workflow.

`release-prepare` needs a `RELEASE_TOKEN` secret with `contents: write` and
`pull-requests: write`. GitHub schedules no checks on a pull request opened by
the default `GITHUB_TOKEN`, and the ruleset would never be satisfiable.

`make release-dry-run` runs the same builds and releases nothing.
