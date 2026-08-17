# Changelog

What changed in each release, newest first. Versions are [semantic
versioning](https://semver.org): while scriv is `0.x`, a new command or flag is
a minor and a fix is a patch.

The entry for a version is what its GitHub release says, so it is written for
someone deciding whether to upgrade, not for someone reading the diff. Its
heading is that release's title, which is why the date sits on a line below
instead of in it: GitHub prints the date beside the title already. Both lines
are written by `.github/date-changelog.sh` when the release pull request is
raised, never by hand.

## Unreleased

## v0.7.2

*Released 2026-08-17*

### Changed

- `scriv history` no longer offers scriv's own key bindings back. Pressing
  ctrl-o records `scriv-repo-cd` in fish's history, so the rows at the top of
  ctrl-r were the keys you had just pressed rather than anything you typed.
- `scriv pr` says it is not in a git repository itself, rather than passing on
  `gh`'s report of git's `fatal: not a git repository`. `GH_REPO` still names a
  repository to work on without one.
- `scriv pr` says when a listing stopped at `--limit`, as `repo clone` already
  did, so a missing pull request is not mistaken for one that is not there.
- A missing `root` now points at the config file to edit when there is one, and
  only sends you to `scriv config init` when there is not — that command refuses
  to overwrite a config, so the old advice was a dead end.
- `scriv config check` reports whether `gh` is still logged in. An expired token
  breaks `pr` as completely as a missing `gh` does, and looked like nothing at
  all. It is the one check that waits on the network.

## v0.7.1

*Released 2026-08-17*

### Changed

- Listings write in blocks rather than a line at a time, which halves
  `scriv history ls` over a long fish history.
- `scriv worktree` asks git one question fewer, taking about a quarter off how
  long ctrl-t takes to open.
- `scriv edit dir` no longer builds a preview command for every directory it
  finds, only for the one you are looking at — the walk of a large tree stays
  flat rather than growing a pane per row.

## v0.7.0

*Released 2026-08-17*

### Added

- `scriv pr open --current` opens the pull request for the branch you have
  checked out, and the repository's pull request list when it has none. In
  fish, f2 does it.

### Changed

- `scriv repo clone` shows the date each repository was last pushed to, as a
  column before the description.
- Its rows are no longer coloured end to end. Only the visibility word is
  tinted — yellow private, magenta internal — and a repository you already have
  is marked with a green tick rather than being greyed out.
- The preview pane is gone from that selector: everything it held is a column
  now.

## v0.6.0

*Released 2026-08-17*

### Added

- `scriv repo clone` colours its list by who can see a repository: private is
  yellow, internal magenta, public the terminal's own foreground. A repository
  you already have stays grey.
- `scriv repo clone --archived` lists an owner's archived repositories, which
  are now left out by default.

## v0.5.1

*Released 2026-08-13*

### Added

- An install script, published with every release:
  `curl -LsSf https://github.com/joakimen/scriv/releases/latest/download/scriv-installer.sh | sh`.
  It installs into `~/.local/bin`.

## v0.5.0

*Released 2026-08-12*

### Added

- `scriv worktree` lists and selects the working trees of the repository you are
  standing in. In fish, ctrl-t jumps to one.

### Changed

- A release is cut by pushing a tag, rather than by dispatching a workflow that
  needed a personal access token to open its own pull request.

## v0.4.0

*Released 2026-08-11*

### Changed

- scriv builds for Apple Silicon and refuses to compile anywhere else. Signal
  numbers past the POSIX five differ between Darwin and Linux — 19 is `CONT` on
  one and `STOP` on the other — so a binary for the wrong platform resumed the
  process it was told to suspend.

## v0.3.2

*Released 2026-08-11*

### Changed

- Releases are built and published by GitHub Actions. Nothing is compiled or
  signed on a maintainer's machine.

## v0.3.1

*Released 2026-08-11*

### Changed

- ctrl-q is left unbound, for your own use; `scriv edit` is reached through the
  `fe` function instead.

## v0.3.0

*Released 2026-08-10*

### Added

- `scriv proc` finds a running process and signals it. scriv's own process and
  everything that spawned it are never offered.
- `scriv config check` looks at everything scriv depends on in one pass and says
  what is wrong with each.
- `scriv file prune` drops tracked files that no longer exist.
- `scriv edit dir` selects a directory, splitting `edit` into `file` and `dir`.
- `--color auto|always|never`, and `SCRIV_NO_COLOR` under `auto`.

### Changed

- `pick` is now `sel` and `remove` is `rm`, and each group takes a one-letter
  abbreviation: `r`, `f`, `b`, `w`, `e`, `h`, `c`, and `pc` for `proc`.
- A development build reports `<version>-dev.<sha>`, so it cannot be mistaken
  for the release of the same name.

### Fixed

- A selector no longer spins at full CPU after its terminal goes away.
- A key binding no longer draws over the output it just produced.

## v0.2.1

*Released 2026-07-30*

### Added

- `scriv history` searches fish's history, newest first with repeats collapsed,
  each row dated by when you last ran it.

### Fixed

- A listing ends quietly when its reader stops reading, so `scriv history ls |
  head` no longer ends in a panic.

## v0.2.0

*Released 2026-07-30*

Rewritten in Rust, with the fuzzy finder compiled in rather than shelled out to.

### Added

- `scriv branch` and `scriv pr` select branches and GitHub pull requests, with
  preview panes and ctrl-r to refresh the list in place.
- `scriv edit` finds a file below `$PWD` and opens it in `$EDITOR`.
- `scriv repo clone` clones into `<root>/<owner>/<repo>`, and `scriv repo open`
  opens a repository's GitHub page.
- `scriv file` takes over the standalone `kf` tool, migrating its list on first
  use.
- `scriv init fish` emits helper functions, key bindings and completions.

### Changed

- Repositories live under one `root` laid out as `<owner>/<repo>`, and labels
  name owners rather than paths. A config in the old shape is refused with the
  replacement written out.

## Earlier releases

0.1.0 and 0.0.1 were the Go tool that only discovered repositories. Their
history is in git.
