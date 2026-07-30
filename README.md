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

![scriv: check out a remote branch, find and open a file, list pull requests](docs/demo.gif)

| | |
| --- | --- |
| **repos** | pick one of your repositories and `cd` into it, open it on GitHub, or clone new ones |
| **files** | keep a list of files you return to, and open one |
| **editing** | fuzzy-find a file where you are standing and open it in `$EDITOR` |
| **branches** | switch to a local branch, or check out a remote one |
| **pull requests** | see what is green, then check one out, open it, or merge it |

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
scriv config print         # set your root, then check it
scriv repo ls              # see what scriv finds under it
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

`scriv repo clone` adds to that first list: pick a GitHub owner — suggested from
your config, the owners already under your root, and your own account, with
anything you type accepted — then fuzzy-select one or more of their repositories.
`tab` selects several and they clone concurrently. Repositories you already have
stay in the list, greyed and marked `✓`, and are skipped rather than re-cloned.
Everything lands at `<root>/<owner>/<repo>`, so a clone is in `repo pick` the
moment it finishes.

```fish
scriv repo clone                # pick an owner, then repositories
scriv repo clone capralifecycle # scope to one owner (any owner, not just yours)
scriv repo clone tailscale/tailscale   # skip both pickers
```

`scriv repo open` opens a repository's GitHub page. Standing in one, that is the
one it opens — there is nothing to ask. Anywhere else it picks from that same
list, and `--pick` asks even from inside a repository, for when you want
somebody else's. Either way it goes through `gh repo view --web`, which reads
the page from that checkout's git remotes, so a directory renamed on clone and a
fork both land where they should.

`scriv file add`/`remove` maintain the tracked list, and `scriv config` /
`scriv init` handle setup. `branch` abbreviates to `br`, `checkout` to `co`,
`ls` to `list`. Any command takes `--help` for its flags.

Branch listings lead with the branch you are on, then the branches in this
clone, then the ones that only exist on a remote — each block most recently
committed to first, so what you were last working on is at the top and
somebody else's month-old remote branch is not.

`scriv pr` has two more verbs of its own — `open` puts a pull request in the
browser, `merge` merges it — and both fuzzy-pick when given no number:

```fish
scriv pr open               # pick one, open it on github.com
scriv pr merge              # pick one; gh asks how to merge
scriv pr merge --squash -d  # squash it and delete the source branch
```

Every pull request list carries its CI — `scriv pr ls` and every `pr` picker,
not just `--status`:

```
#128  ✓    Add a token bucket per API key  @ada
#127  ✗    Round partial usage up  @grace
#126  ⧗ ⊘  Cache quotas in redis  @ada
```

Green `✓` checks passed, red `✗` one failed, yellow `⧗` still running, and `⊘`
conflicts with the base branch. It costs no extra request — the checks come back
with the same `gh pr list` that fetches the titles — and a column appears only
when some pull request in the list has something to put in it, so a repository
with no CI shows neither. Merging cleanly is deliberately left unmarked: it is
true of nearly every row, and GitHub reports it as unknown until a background
job has run, and always for a pull request that is already merged. The preview
spells all of it out in words and names the checks that are failing or still
running.

The marks are shapes, not just colours, so a piped or `NO_COLOR` listing says
exactly as much as a coloured one — and each is a single terminal column in
every locale, so the titles line up whatever the row happens to report.

`scriv pr merge` goes further and colours the whole row by whether it can
actually go in — green ready, yellow waiting on checks, red blocked by a failure
or a conflict, grey for a draft or something already closed. Colouring by state
there would paint a list of open pull requests one uniform green, exactly where
the colour is worth the most.

`scriv edit` is the one verb rather than a noun: it searches the directory
you are in — not a list scriv keeps — and opens what you choose.

```fish
scriv edit              # pick a file below $PWD, open it in your editor
scriv edit --tracked    # pick from your tracked files instead
scriv edit src/main.rs  # skip the picker
```

Select several with `tab` and they open together. The walk honours
`.gitignore`, `.ignore` and `.fdignore`, streams into the picker as it goes —
so a directory of a million files is typeable in milliseconds, not once the
last one is found — and quietly skips paths it is not allowed to read. The
editor is `$VISUAL`, then `$EDITOR`. `edit` abbreviates to `e`.

Every `pick` prints one line and nothing else, so it composes:

```fish
cd (scriv repo pick)
gh pr view (scriv pr pick)
```

Previews show commits and working-tree status for repositories and branches,
title, failing checks and description for pull requests, and contents for
files. They come from data already fetched, or from local commands kept to tens
of milliseconds, so scrolling a long list never stalls.

In a branch or pull request picker, `ctrl-r` reloads the list — `git fetch --all
--prune` for branches, another `gh pr list` for pull requests. The picker never
closes: the header says `⟳ refreshing…` while the work runs in the background,
and the rows are replaced underneath your query and cursor when it lands. Those
are the two lists that go stale while you read them — a colleague pushes, a check
goes green. Neither command's output reaches your terminal; a failure leaves the
list alone and is reported once you are out of the picker.

The pickers over local data have nothing to re-ask, so they leave `ctrl-r` to
skim, where it switches between fuzzy and regex matching. Waits that happen
before a picker can open — `--fetch`, the first `gh pr list` — get a spinner on
stderr instead, so a slow network never looks like a hang.

## Shell integration (fish)

```fish
scriv init fish | source

function fish_user_key_bindings
    scriv_key_bindings
end
```

| Key | Does |
| --- | --- |
| `ctrl-o` | pick a repository and `cd` into it |
| `ctrl-g` | pick a branch and check it out |
| `ctrl-q` | pick a file below `$PWD` and open it in `$EDITOR` |
| `f1` | open this repository on GitHub, or pick one when you are not in one |
| `f3` | pick a tracked file and open it in `$EDITOR` |
| `f7` | pick a pull request and check it out |

Of those, `ctrl-o` and `ctrl-q` are unbound in fish, and `ctrl-g` replaces
`cancel`, which `escape` and `ctrl-c` also do. The function keys replace nothing
— fish binds none of them itself. `f1` and `f3` sit at the low end that users'
own tools tend to leave alone, and `f7` steps over `f4`/`f5`, where those tools
cluster. `ctrl-p` is left alone on purpose: it is fish's `up-line`, which is how
history gets walked on a one-line prompt. alt is left untouched too, since fish
binds most of it already.

It also defines `fe` — find, fuzzy-pick, edit — as a short alias for
`scriv edit`, forwarding its arguments so `fe -t` and `fe src/main.rs` work
too. That is the only unprefixed name scriv defines; redefine it after sourcing
if you have your own.

To use your own keys, bind them after `scriv_key_bindings` — the last binding
for a key wins. For other shells, `scriv init bash`/`zsh`/`powershell`/`elvish`
emit completions.

## Configuration

`scriv config init` writes `~/.config/scriv/config.toml`:

Settings are grouped by the command that reads them. A key lives in a command's
table when exactly one command reads it; anything genuinely shared — the picker
— stays at the top level.

```toml
# `scriv repo`: where your repositories are, and how they are labelled.
[repo]

# Every repository lives under one root, laid out as <owner>/<repo> — the same
# shape as GitHub itself. `repo clone` writes here, so a clone always lands
# somewhere `repo pick` will find it.
root = "~/dev/github.com"

# Repositories outside the root, listed one at a time. An escape hatch for
# checkouts that predate the layout; `clone` never writes here.
extra = ["~/bin"]

# Directory names to skip while searching.
ignore = ["node_modules", "target"]

# Repo path rendering: relative | tilde | full
display = "relative"

# Labels name owners, one label to many owners, so everything you touch for work
# colours as one group however many orgs it spans. `work` is cyan and `personal`
# green wherever they appear; any other label takes one of the remaining hues. An
# owner with no label still shows up, in the terminal's ordinary foreground.
labels = { personal = ["joakimen"], work = ["capralifecycle", "nsbno"] }

# The built-in fuzzy picker, shared by every command that opens one.
[picker]
height = "50%"                # finder height
preview = true                # show a preview pane for the highlighted row
preview_window = "right:50%"  # preview layout
```

`labels` is written inline, on one line, rather than as a `[repo.labels]`
header. Both parse the same, but a header captures every bare key written after
it — add `ignore` below a `[repo.labels]` header and it silently becomes a label
named "ignore" instead of an error. The inline table has no such ordering rule.

One root, always two levels deep. The depth is not a setting because it is not a
preference: the root mirrors GitHub's own namespace, and fixing it is what lets
`repo clone` work out where a repository belongs without being told. Labels name
*owners* rather than directories, so moving an org between `work` and `personal`
is a one-word edit instead of a reshuffle on disk.

The editor is `$VISUAL`, then `$EDITOR`. There is no config key for it: it is
already stated once where every other terminal tool reads it, and a third place
to set it is a third place to forget it is set.

A config still using an older layout — top-level `root`/`owners`, or the
`[[paths.*]]` format before that — is refused with the replacement written out
for you, derived from what was there.

The tracked-files list sits beside the config in `~/.config/scriv/files`, one
path per line, managed by `scriv file add`/`remove`.

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
