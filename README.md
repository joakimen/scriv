# scriv

[![ci](https://github.com/joakimen/scriv/actions/workflows/ci.yml/badge.svg)](https://github.com/joakimen/scriv/actions/workflows/ci.yml)
[![release](https://img.shields.io/github/v/release/joakimen/scriv?logo=github&color=blue)](https://github.com/joakimen/scriv/releases/latest)
[![platform](https://img.shields.io/badge/platform-macOS%20%28Apple%20Silicon%29-blue?logo=apple&logoColor=white)](#install)
[![license](https://img.shields.io/github/license/joakimen/scriv?color=blue)](LICENSE)

![A candle burning above an open book, flanked by engraved rules](docs/art/banner.svg)

Provides fuzzy-completion for various local and remote resources.

A single fuzzy selector over resources that would otherwise each need their own
command and output parser: Git repositories, tracked files, Markdown notes,
branches, worktrees, GitHub pull requests, system processes and fish history.
Each group lists its set, selects from it, and acts on the selection.

The finder is linked into the binary — no `fzf` dependency and no subprocess.
`scriv init fish` emits shell functions, key bindings and completions.

![scriv: check out a remote branch, find and open a file, list pull requests](docs/demo.gif)

## Install

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/joakimen/scriv/releases/latest/download/scriv-installer.sh | sh
```

The script installs into `~/.local/bin` and adds it to `PATH`. Alternatively,
through a version manager or from source:

```sh
mise use -g github:joakimen/scriv
cargo install --git https://github.com/joakimen/scriv
```

Supported platform: macOS on Apple Silicon. `scriv proc` depends on Darwin's
signal numbers, and the crate refuses to compile for any other target.

External requirements: `git`, plus `gh` for `pr` and `repo clone`/`open`, a
fish history file for `history`, and `$VISUAL` or `$EDITOR` for `edit` — or
`[note] editor` for `note`. `scriv config check` reports on each.

## Setup

```sh
scriv config init          # write ~/.config/scriv/config.toml, then set `root`
scriv init fish | source   # helpers, key bindings, completions
scriv config check         # confirm it all resolves
```

Add the second line to `~/.config/fish/config.fish` to make it permanent. Key
bindings are emitted as a function rather than bound at source time, so they
compose with fish's binding lifecycle; call it from `fish_user_key_bindings`:

```fish
function fish_user_key_bindings
    scriv_key_bindings
end
```

`config init` writes every setting as a commented line. `repo.root` is the one
that must be set: repositories are located at `<root>/<owner>/<repo>`.

```toml
[repo]
root = "~/dev/github.com"
ignore = ["node_modules", "target"]
# labels = { personal = ["your-github-user"], work = ["acme", "acme-labs"] }

[note]
root = "~/notes"    # an Obsidian vault, or any tree of Markdown files
editor = "nvim"     # what `note edit` opens one with; unset, $VISUAL / $EDITOR
```

## Commands

| Group | Verbs |
| --- | --- |
| `scriv repo` | `ls` `sel` `open` `clone` |
| `scriv file` | `ls` `sel` `add` `rm` `prune` |
| `scriv note` | `ls` `sel` `edit` |
| `scriv branch` | `ls` `sel` `checkout` `rm` |
| `scriv worktree` | `ls` `sel` `add` `rm` |
| `scriv pr` | `ls` `sel` `checkout` `open` `merge` |
| `scriv proc` | `ls` `sel` `kill` |
| `scriv history` | `ls` `sel` |
| `scriv edit` | `file` `dir` — found below `$PWD`, opened in `$EDITOR` |
| `scriv config` | `init` `print` `path` `check` |
| `scriv init` | `fish` and every other shell — see Setup |

`ls` prints the set, `sel` fuzzy-selects one entry, and the remaining verbs act
on the selection. `sel` prints to stdout and composes: `cd (scriv repo sel)`.
Groups abbreviate to one letter — `r`, `f`, `n`, `b`, `w`, `e`, `h`, `c`, and
`pc` for `proc`.

## Key bindings

`scriv_key_bindings` binds these in fish:

| Key | Action |
| --- | --- |
| `ctrl-o` | `cd` to a repository |
| `ctrl-t` | `cd` to a worktree of the current repository |
| `ctrl-g` | check out a branch |
| `ctrl-r` | search shell history onto the command line |
| `up` | the same, on the first line of a prompt |
| `f1` | open a repository on GitHub |
| `f2` | open this branch's pull request, or the list if it has none |
| `f3` | open a tracked file in `$EDITOR` |
| `f7` | check out a pull request |
| `f10` | open a note from your vault |

`scriv init fish` also defines `fe` (`scriv edit`, arguments passed through) and
`kl` (`scriv proc kill --force`).

Inside a selector, `ctrl-v` hides and shows the preview pane and `tab` takes
several rows where several are allowed. Anything else a selector answers to is
named in its own header — `f2` and `f7` mean the same in a pull request list as
they do at the prompt, and `f1` does in a repository list.

Flags are documented in `scriv --help`, settings in the generated
`config.toml`.

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
says so with a `Release: minor` line in a commit message on its branch, which
`release-plz.toml` matches. Not the pull request description: squash merges here
take the commit messages and discard it. Should the open pull request read the
wrong version anyway, edit `Cargo.toml` on its branch, run `cargo check` for the
lockfile, fix the `CHANGELOG.md` heading to match, and merge before anything
else lands: the next push to `main` rebuilds that branch from scratch.

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
