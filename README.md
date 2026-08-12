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

macOS on Apple Silicon. Needs `git`; `gh` for pull requests and
`repo clone`/`open`.

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
| `scriv worktree` | `ls` `sel` |
| `scriv pr` | `ls` `sel` `checkout` `open` `merge` |
| `scriv proc` | `ls` `sel` `kill` |
| `scriv history` | `ls` `sel` |
| `scriv edit` | `file` `dir` — found below `$PWD`, opened in `$EDITOR` |
| `scriv config` | `init` `print` `path` `check` |
| `scriv init` | `fish` and every other shell — see Setup |

`ls` prints the set, `sel` fuzzy-selects one line of it, and the other verbs
act. Every `sel` composes: `cd (scriv repo sel)`. Each group takes a one-letter
abbreviation — `r`, `f`, `b`, `w`, `e`, `h`, `c`, and `pc` for `proc`.

In fish, `ctrl-r` searches history, `ctrl-o` jumps to a repository, `ctrl-t` to
a worktree of the one you are in, and `ctrl-g` checks out a branch; `f1`, `f3`
and `f7` open a repository, a tracked file and a pull request.

`scriv --help` covers the flags, the generated `config.toml` the settings.

## Development

```sh
make              # fmt check, clippy, tests, release build
make hooks        # install the git hooks in prek.toml
make demo         # re-record docs/demo.gif
make demo-fixture # build the demo sandbox and poke at it by hand
```

## Releasing

A pushed tag is the release. The version bump reaches `main` as an ordinary
pull request first, because the ruleset there requires the `build` and `demo`
checks like it does of anything else.

```sh
git switch -c release/v0.5.0
cargo release version minor --execute  # writes Cargo.toml and Cargo.lock
cargo release replace --execute        # dates the CHANGELOG.md heading
# ...commit, open the pull request, merge it...
git switch main && git pull
cargo release tag --execute && cargo release push --execute
```

That last line tags the merged commit `v0.5.0` and pushes the tag; `git tag
v0.5.0` and `git push origin v0.5.0` do the same thing. cargo-release is pinned
in `mise.toml` and configured under `[package.metadata.release]` in
`Cargo.toml`.

The tag starts [dist](https://axodotdev.github.io/cargo-dist), configured in
`dist-workspace.toml`: it refuses a version no package carries, builds
`aarch64-apple-darwin`, writes the install script, and publishes the release
once the tarball, its checksum and its build provenance exist. The release notes
are that version's section of `CHANGELOG.md`, so an entry written under
`## Unreleased` while the work was done is what a reader sees.
`.github/workflows/release.yml` is generated — change `dist-workspace.toml` and
run `dist init`, never the workflow.

No secret and no token beyond the workflow's own `GITHUB_TOKEN`. Every pull
request runs `dist plan`, which catches a misconfiguration before a tag exists.
