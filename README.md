# scriv

[![ci](https://github.com/joakimen/scriv/actions/workflows/ci.yml/badge.svg)](https://github.com/joakimen/scriv/actions/workflows/ci.yml)

> "We found him wandering around, with a candle."

## Summary

[Scriv](https://kingkiller.fandom.com/wiki/Scriv) is a CLI for finding your way
around a machine. Each of the things it does comes with a built-in fuzzy picker:

- **repos** — discover your Git repositories and jump into one
- **files** — track the config and notes files you return to, and open one
- **branches** — switch between local branches and check out remote ones
- **pull requests** — pick a GitHub PR and check it out

It is self-contained: the fuzzy finder ([skim](https://github.com/skim-rs/skim))
and the file walker (the [`ignore`](https://docs.rs/ignore) crate that powers
[fd](https://github.com/sharkdp/fd)) are compiled in. No `fzf`, `fd`, or other
external tools are required. Branch commands shell out to `git`; the pull
request commands shell out to [`gh`](https://cli.github.com), reusing whatever
`gh auth login` already set up — scriv stores no tokens of its own.

## Commands

- `scriv repo ls [-A]` — list discovered repositories (`-A` for absolute paths)
- `scriv repo pick` — fuzzy-select a repository (colour-tagged by group), print
  its absolute path
- `scriv file ls [--status] [--missing|--exists]` — list known files
- `scriv file pick` — fuzzy-select a known file, print its absolute path
- `scriv file add [path]` — add a file; omit the path to pick one from the
  current directory
- `scriv file remove [path]` — remove a file; omit the path to pick interactively
- `scriv branch ls [--local|--remote] [--status] [--fetch]` — list branches
- `scriv branch pick` — fuzzy-select a branch, print its name
- `scriv branch checkout [branch]` — check out a branch; omit the name to pick
  one (aliases: `co`, `switch`)
- `scriv pr ls [--state <state>] [--limit <n>] [--status]` — list pull requests
- `scriv pr pick` — fuzzy-select a pull request, print its number
- `scriv pr checkout [number]` — check out a pull request; omit the number to
  pick one
- `scriv config init` — write a starter configuration file
- `scriv config print` — print the resolved configuration
- `scriv config path` — print the configuration file path
- `scriv init <shell>` — print shell integration to `source` (`fish` adds
  pick-and-`cd`/edit helpers and key bindings; `bash`/`zsh`/`powershell`/`elvish`
  emit completions)

`pick` always prints a single absolute path, so it composes cleanly with shell
functions:

```fish
cd (scriv repo pick)
```

`scriv branch` may be abbreviated to `scriv br`.

## Branches

`scriv branch` lists local and remote branches together, most recently
committed to first, and colours each row by where the branch lives:

| Colour | Meaning | `--status` tag |
| ------ | ------- | -------------- |
| green  | exists here **and** on a remote | `both` |
| yellow | exists **only** here — never pushed, or its upstream is gone | `local` |
| cyan   | exists **only** on a remote | `remote` |

A remote-only branch is shown with its remote (`origin/feature`), so two remotes
carrying the same branch stay distinguishable. Checking one out creates the
matching local branch and sets its upstream — the equivalent of
`git switch --track origin/feature` — so `git push` and `git pull` work right
away. The current branch is marked with `*`.

```sh
scriv branch checkout            # pick from every branch and switch to it
scriv branch checkout --fetch    # refresh remotes (pruning deleted) first
scriv branch checkout --remote   # pick only branches that exist on a remote
scriv branch checkout main       # skip the picker
scriv branch ls --status         # marker, tag, last commit
```

`branch ls` prints bare names by default so it pipes cleanly; colour is dropped
when stdout is not a terminal, and `NO_COLOR` is honoured.

## Pull requests

`scriv pr` uses the [`gh`](https://cli.github.com) CLI, so it works wherever
`gh` is authenticated — including GitHub Enterprise — and needs no configuration
in scriv. Rows are coloured by state: green open, grey draft, magenta merged,
red closed.

```sh
scriv pr checkout                # pick an open PR and check it out
scriv pr ls --state all --limit 20
gh pr view (scriv pr pick)       # pick prints the number, so it composes
```

`pr checkout` hands off to `gh pr checkout`, which handles PRs from forks and
sets the upstream.

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
scriv config print         # tune the search paths, then verify
scriv repo ls              # verify discovery
```

## Shell integration (fish)

`scriv init fish` emits helper functions, a key-binding function, and
completions. Source it from `config.fish`:

```fish
scriv init fish | source

function fish_user_key_bindings
    scriv_key_bindings     # ctrl-o / alt-o -> repo cd, f3 -> file edit,
                           # f4 -> branch checkout, f5 -> PR checkout
end
```

This defines `scriv-repo-cd` (pick a repo and `cd` into it), `scriv-file-edit`
(pick a known file and open it in `$EDITOR`), `scriv-branch-checkout`, and
`scriv-pr-checkout`.

For other shells, `scriv init bash` / `zsh` / `powershell` / `elvish` emit
completions you can source or install.

## Configuration

Configuration lives under `$XDG_CONFIG_HOME/scriv` (default
`~/.config/scriv`):

- `config.toml` — hand-edited settings (a legacy `config.json` is still read
  when no TOML file is present)
- `files` — the known-files list, one path per line, managed by
  `scriv file add`/`remove`

Run `scriv config init` to write a starter `config.toml`.

### `config.toml`

```toml
# Directory names to skip while searching.
ignore = ["node_modules", "target"]

# Search paths are grouped by a label. repo pick shows which group a repo
# belongs to (once more than one group is configured).
[[paths.personal]]
path = "~/dev/github.com"
depth = 2

[[paths.personal]]
path = "~/bin"
depth = 0

[[paths.work]]
path = "~/work/acme"
depth = 2

[picker]
height = "50%"                           # built-in finder height
display = "relative"                      # repo path rendering: relative | tilde | full
```

### Configuration keys

#### `paths.<group>`

A named group of search paths. The group label is shown — in a colour assigned
per group — alongside each repo in `repo pick`, so you can tell at a glance
which context a repo belongs to (e.g. `personal` vs a client name). Groups
appear in the order written.

A flat `[[paths]]` list (no group) is also accepted and lands in a `default`
group.

#### `paths.<group>[].path`

Required. The root path under which to search for repos. The root path may
itself be a repo.

#### `paths.<group>[].depth`

Optional (default `0`). The search depth for the associated path. Tune this to
your project layout — it is the primary factor in discovery performance.

- `~/dev/github.com` at depth `2`: `~/dev/github.com/repo1` and
  `~/dev/github.com/dir1/repo1` are returned; `~/dev/github.com/a/b/repo1` is
  not.
- `~/bin` at depth `0`: `~/bin` is returned if it is a repo; `~/bin/repo1` is
  not.

#### `ignore`

Optional. Directory names to skip during search.
Default: `node_modules`, `vendor`, `dist`, `build`, `target`.

#### `picker.height`

Optional (default `"50%"`). Height of the built-in fuzzy finder, e.g. `"50%"`
or `"20"`. The finder is compiled in (skim); there is no `fzf` dependency.

#### `picker.display`

Optional (default `"relative"`). How `repo pick` renders each repository:

- `relative` — path relative to the search root it was found under, so the
  shared base (named by the group) is not repeated on every row:
  `personal  joakimen/scriv`.
- `tilde` — absolute path with the home directory collapsed to `~`:
  `personal  ~/dev/github.com/joakimen/scriv`.
- `full` — the full absolute path.

The selected path is always absolute regardless of this setting; only the
display changes. `repo ls` is unaffected.
