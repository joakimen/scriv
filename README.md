# scriv

[![ci](https://github.com/joakimen/scriv/actions/workflows/ci.yml/badge.svg)](https://github.com/joakimen/scriv/actions/workflows/ci.yml)
[![release](https://img.shields.io/github/v/release/joakimen/scriv?logo=github&color=blue)](https://github.com/joakimen/scriv/releases/latest)
[![platform](https://img.shields.io/badge/platform-macOS%20%28Apple%20Silicon%29-blue?logo=apple&logoColor=white)](#install)
[![license](https://img.shields.io/github/license/joakimen/scriv?color=blue)](LICENSE)

![A candle burning above an open book, flanked by engraved rules](docs/art/banner.svg)

Provides fuzzy-completion for various local and remote resources.

One selector over the things you reach for all day: repositories, tracked
files, branches, worktrees, GitHub pull requests, running processes and shell
history. The finder is compiled in, so a selection is one process rather than a
pipeline, and `scriv init fish` puts the common ones on a key.

![scriv: check out a remote branch, find and open a file, list pull requests](docs/demo.gif)

## Install

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/joakimen/scriv/releases/latest/download/scriv-installer.sh | sh
```

The script puts the binary in `~/.local/bin` and adds that to your `PATH`. Two
other ways in, resolving the same release archive or building it yourself:

```sh
mise use -g github:joakimen/scriv
cargo install --git https://github.com/joakimen/scriv
```

macOS on Apple Silicon, and nothing else — the signal table `scriv proc` uses
is Darwin's. `git` is required. `gh` is needed by `pr` and by `repo
clone`/`open`, fish's history file by `history`, and `$VISUAL` or `$EDITOR` by
`edit`; `scriv config check` reports on each.

## Setup

```sh
scriv config init          # write ~/.config/scriv/config.toml, then set `root`
scriv init fish | source   # helpers, key bindings, completions
scriv config check         # confirm it all resolves
```

Put the middle line in `~/.config/fish/config.fish` to keep it. The bindings
arrive as a function rather than bound at source time, so they compose with
fish's own lifecycle — call it yourself to switch them on:

```fish
function fish_user_key_bindings
    scriv_key_bindings
end
```

Every setting is a commented line in the file `config init` writes. The one
that matters is the root your repositories live under, laid out `<owner>/<repo>`
as GitHub itself is:

```toml
[repo]
root = "~/dev/github.com"
ignore = ["node_modules", "target"]
# labels = { personal = ["your-github-user"], work = ["acme", "acme-labs"] }
```

## Commands

| Group | Verbs |
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

## Key bindings

`scriv_key_bindings` binds these in fish:

| Key | Action |
| --- | --- |
| `ctrl-o` | `cd` to a repository |
| `ctrl-t` | `cd` to a worktree of the one you are in |
| `ctrl-g` | check out a branch |
| `ctrl-r` | search shell history onto the command line |
| `up` | the same, on the first line of a prompt |
| `f1` | open a repository on GitHub |
| `f3` | open a tracked file in `$EDITOR` |
| `f7` | check out a pull request |

Two functions come with them: `fe` for `scriv edit`, arguments passed through,
and `kl` for `scriv proc kill --force`.

`scriv --help` covers the flags, the generated `config.toml` the settings.

## Development

```sh
make              # fmt check, clippy, tests, release build
make hooks        # install the git hooks in prek.toml
make demo         # re-record docs/demo.gif
make demo-fixture # build the demo sandbox and poke at it by hand
```

## Releasing

A pushed tag is the release, and merging a pull request is what pushes it.
Nothing is run by hand.

Every push to `main` runs [release-plz](https://release-plz.dev), which keeps a
pull request open proposing the next version: the bump in `Cargo.toml` and
`Cargo.lock`, and the version heading and release date that `## Unreleased`
work in `CHANGELOG.md` is filed under.
Merging that pull request tags the merge commit with that version and pushes
the tag. Leaving it open holds the version, which is the answer to a run of
pull requests that only touched docs, `demo/` or CI.

The proposal is a patch. A change that earns a minor — a new command or flag —
says so with a `Release: minor` line in the body it is squashed with, which
`release-plz.toml` matches. Should the open pull request read the wrong version
anyway, edit `Cargo.toml` on its branch, run `cargo check` for the lockfile, fix
the `CHANGELOG.md` heading to match, and merge before anything else lands: the
next push to `main` rebuilds that branch from scratch.

The tag starts [dist](https://axodotdev.github.io/cargo-dist), configured in
`dist-workspace.toml`: it refuses a version no package carries, builds
`aarch64-apple-darwin`, writes the install script, and publishes the release
once the tarball, its checksum and its build provenance exist. The release takes
its title and notes from that version's section of `CHANGELOG.md`, so an entry
written under `## Unreleased` while the work was done is what a reader sees.
`.github/workflows/release.yml` is generated — change `dist-workspace.toml` and
run `dist init`, never the workflow.

dist needs no secret, and every pull request runs `dist plan`, which catches a
misconfiguration before a tag exists. release-plz needs one: `RELEASE_PLZ_TOKEN`,
a fine-grained token with `Contents` and `Pull requests` write on this
repository. The workflow's own `GITHUB_TOKEN` cannot stand in — a tag it pushes
starts no workflow, so dist would never build, and a pull request it opens runs
no checks, so `build` and `demo` would never report to the ruleset on `main`.

## License

[MIT](LICENSE)
