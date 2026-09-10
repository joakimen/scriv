# scriv

[![ci](https://github.com/joakimen/scriv/actions/workflows/ci.yml/badge.svg)](https://github.com/joakimen/scriv/actions/workflows/ci.yml)
[![release](https://img.shields.io/github/v/release/joakimen/scriv?logo=github&color=blue)](https://github.com/joakimen/scriv/releases/latest)
[![platform](https://img.shields.io/badge/platform-macOS%20%28Apple%20Silicon%29-blue?logo=apple&logoColor=white)](#install)
[![license](https://img.shields.io/github/license/joakimen/scriv?color=blue)](LICENSE)

![A candle burning above an open book, flanked by engraved rules](docs/art/banner.svg)

Provides fuzzy-completion for various local and remote resources.

One fuzzy selector over things that would otherwise each need their own command
and output parser. Every group lists its set, selects from it, and acts on the
selection: `ls` prints, `sel` picks one, and the rest act. `sel` writes to
stdout and composes — `cd (scriv repo sel)`. The finder is linked into the
binary, so there is no `fzf` subprocess.

![scriv: check out a remote branch, find and open a file, list pull requests](docs/demo.gif)

## Install

macOS on Apple Silicon.

```sh
# installer script — writes to ~/.local/bin and adds it to PATH
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/joakimen/scriv/releases/latest/download/scriv-installer.sh | sh

# version manager
mise use -g github:joakimen/scriv

# from source
cargo install --git https://github.com/joakimen/scriv
```

## Setup

```sh
scriv config init     # write ~/.config/scriv/config.toml, then set `root`
scriv init <shell>    # shell functions, key bindings and completions, to source
scriv config print    # every setting, and what is in force for it
scriv config check    # what scriv reaches for, and what is missing
```

## Commands

| Command | |
| --- | --- |
| `scriv repo` | your Git repositories, laid out as `<root>/<owner>/<repo>` |
| `scriv file` | the files you keep coming back to, wherever they live |
| `scriv edit` | a file or directory below `$PWD`, opened in `$EDITOR` |
| `scriv note` | the Markdown notes in your vault — by name, or by what they say |
| `scriv branch` | this repository's branches, local and remote |
| `scriv worktree` | this repository's worktrees |
| `scriv pr` | this repository's GitHub pull requests |
| `scriv ps` | what is running, and what to kill |
| `scriv history` | the commands you have already run, back onto the command line |
| `scriv project` | builds or installs `$PWD`, whatever it turns out to be written in |
| `scriv config` | write, read back and check the configuration |
| `scriv stats` | what you run, how often, and how long scriv takes over it |
| `scriv init` | the shell integration below |

Verbs and flags are in `scriv <command> --help`; settings are described in the
generated `config.toml`.

## Key bindings

scriv binds no key of its own. `[shell.bindings]` maps a key to an action and
`[shell.aliases]` maps a name to one — neither holds shell code, so one table
serves every shell scriv can write for. `scriv config init` writes a set out
commented, to uncomment and edit:

```toml
[shell.bindings]
ctrl-o = "repo-cd"          # cd to a repository
ctrl-t = "worktree-cd"      # cd to a worktree of this repository
ctrl-g = "branch-checkout"  # check out a branch
f10    = "note-edit"        # open a note from the vault

[shell.aliases]
fe = "edit"                 # scriv edit
b  = "project-build"        # scriv project build
```

What a table holds is the whole of what is bound, so leaving a key out is how
it stays free. `scriv config print` lists what you have with what each action
does, and `scriv config check` says whether they all resolve — an action scriv
does not define stops `scriv init` rather than emitting a shell where one key
silently does nothing.

Inside a selector, `ctrl-v` hides and shows the preview pane and `tab` takes
several rows where several are allowed. Anything else is named in the
selector's own header.

## Development

```sh
make              # fmt check, clippy, tests, release build
make hooks        # install the git hooks in prek.toml
make demo         # re-record docs/demo.gif
make demo-fixture # build the demo sandbox and poke at it by hand
```

Dependency updates come from [Renovate](https://docs.renovatebot.com),
configured in `.github/renovate.json5`.

## Releasing

Merging the [release-plz](https://release-plz.dev) pull request tags the merge commit, and [dist](https://axodotdev.github.io/cargo-dist) builds and publishes the release from that version's section of `CHANGELOG.md`.

## License

[MIT](LICENSE)
