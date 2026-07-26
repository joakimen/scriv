# scriv

[![ci](https://github.com/joakimen/scriv/actions/workflows/ci.yml/badge.svg)](https://github.com/joakimen/scriv/actions/workflows/ci.yml)

> "We found him wandering around, with a candle."

## Summary

[Scriv](https://kingkiller.fandom.com/wiki/Scriv) is a CLI for finding your way
around a machine. It does two things, each with a built-in fuzzy picker:

- **repos** — discover your Git repositories and jump into one
- **files** — track the config and notes files you return to, and open one

It is self-contained: the fuzzy finder ([skim](https://github.com/skim-rs/skim))
and the file walker (the [`ignore`](https://docs.rs/ignore) crate that powers
[fd](https://github.com/sharkdp/fd)) are compiled in. No `fzf`, `fd`, or other
external tools are required.

## Commands

- `scriv repo ls [-A]` — list discovered repositories (`-A` for absolute paths)
- `scriv repo pick` — fuzzy-select a repository (tagged by group), print its
  absolute path
- `scriv file ls [--status] [--missing|--exists]` — list known files
- `scriv file pick` — fuzzy-select a known file, print its absolute path
- `scriv file add [path]` — add a file; omit the path to pick one from the
  current directory
- `scriv file remove [path]` — remove a file; omit the path to pick interactively
- `scriv config init` — write a starter configuration file
- `scriv config print` — print the resolved configuration
- `scriv config path` — print the configuration file path
- `scriv init fish` — print shell integration to `source`

`pick` always prints a single absolute path, so it composes cleanly with shell
functions:

```fish
cd (scriv repo pick)
```

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
    scriv_key_bindings     # ctrl-o / alt-o -> repo cd, f3 -> file edit
end
```

This defines `scriv-repo-cd` (pick a repo and `cd` into it) and
`scriv-file-edit` (pick a known file and open it in `$EDITOR`).

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
```

### Configuration keys

#### `paths.<group>`

A named group of search paths. The group label is shown alongside each repo in
`repo pick`, so you can tell at a glance which context a repo belongs to (e.g.
`personal` vs a client name). Groups appear in the order written.

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
