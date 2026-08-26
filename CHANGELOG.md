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

### Added

- `[selector] preview_theme` picks the `bat` theme every file preview is drawn
  with, and defaults to Catppuccin Mocha. It is passed as `--theme`, so it wins
  over `BAT_THEME` and your own `bat` config — a preview pane is scriv's to make
  legible, and a theme chosen for reading whole files in a pager is not always
  one. A theme your `bat` does not know draws in its default instead of failing.
  `config check` now reports whether `bat` is installed at all.

### Changed

- A note preview shows the note. It used to show a summary scriv had assembled
  — the front matter read out, the body redrawn with headings and tags coloured
  in — and a preview that has rearranged what it is previewing is a preview of
  something else. Every note pane is now the file itself, through the same
  `bat` every other preview in scriv goes through, and a `note rg` hit opens at
  its line with that line marked.
- `note cleanup` reads down its list. Candidates are grouped by what is wrong
  with them — the empty ones first, then the untitled, then the ones with no
  name — and smallest first within each group, since size is what separates a
  note that was abandoned from one that was written and never titled. Sizes are
  right-aligned in one letter, `b`/`k`/`M`, so the units stack into a column
  instead of wandering with the length of each number.
- `note cleanup` never offers the scratch note. Being empty is what a scratch
  note is *for*, so it would have been on the list every single run.

## v0.11.1

*Released 2026-08-25*

### Fixed

- `scriv note cleanup` crashed on a vault whose note names are not all English.
  It compared the first eight *bytes* of a name against `untitled`, and the
  eighth byte of a name like `Oppgaveøkt` is the middle of the `ø` rather than
  the end of a letter — which Rust refuses to slice at, taking the whole
  command down with it. Names are now read a character at a time.

## v0.11.0

*Released 2026-08-25*

### Added

- `scriv note scratch` opens the one permanent note that is filed nowhere —
  somewhere to put a thought without first deciding whether it is worth a note
  of its own, and somewhere to find it again afterwards. The same file every
  time: `[note] scratch`, or `scratch/scratch.md`.
- `scriv note cleanup` goes through the notes that were never really written
  and deletes the ones you agree about. Three kinds and no more: a note with
  nothing in it, one still called `Untitled`, and one whose name has no letters
  in it and whose front matter gives it no title either. Each row says which it
  is, the preview shows what is in it, `tab` takes several, and what you chose
  is printed before you are asked — nothing goes without being seen first.
- `note rg` now matches fuzzily: the letters you typed, in order, anywhere on
  the line, so `errhand` finds "error handling". `ctrl-x` searches for the query
  exactly instead — for a phrase, a path, a snippet of code — and `ctrl-f` goes
  back. The header says which is in force.

### Changed

- A note row is a date and a name again. The day it was created leads it, in
  its own colour and never searched, and the name follows tinted by the label
  its directory carries. The columns of tags, folders and task counts are gone
  from the row and remain in the preview pane, which has the width for them.
- `scriv note ls` prints absolute paths, one per line, so the listing pipes
  into whatever reads paths. `--absolute-paths` is gone with nothing left to
  do, and `--status` — the listing a person reads rather than a pipe — collapses
  your home directory to `~` instead of repeating it down every row.
- `scriv note edit` warns when the note you named is not there, rather than
  letting the editor open a new empty buffer and say nothing about it.

### Fixed

- The preview pane no longer keeps the last match on screen after the query
  stops matching anything. It described a note that was no longer in the list.

## v0.10.0

*Released 2026-08-25*

### Added

- `scriv note new` starts a note and drops you straight into your editor. It
  asks nothing first — being asked to name a note is being asked what it is
  about before writing it — so with no name it calls the file after the date and
  time, to the minute. A name with a `/` in it makes the directory. The file
  itself is left for the editor to write, so a note started and abandoned is one
  that never existed rather than an empty one in every listing after it.
- `scriv note rg` searches inside every note as you type. The query goes to
  ripgrep rather than to the fuzzy matcher, so the list is every matching *line*
  in the vault, rebuilt on each keystroke, with the note around the match in the
  preview pane. `tab` takes several; what you pick opens at its line and the
  rest land in the quickfix list behind it. `ctrl-q` switches to filtering what
  came back.
- `[note] labels` names the directories directly below your vault, one label to
  many directories — the same shape as `[repo] labels`, and the same colours, so
  `work` reads the same in both. Label two of five directories and the other
  three still show up, under their own names.

### Changed

- A note row is laid out differently. The note's own name comes first, where it
  is what the eye runs down and what the query matches, and everything else is
  an attribute in a column of its own with a colour that says which: the label
  or directory it is filed under, the folders below that, its tags, how many of
  its tasks are still open, and — no longer leading the row — how long ago it
  was modified and created. The dates and the task count are drawn but never
  searched, so typing `3` finds the note you meant rather than every note that
  is three days old. A column nothing in your vault fills is not drawn at all.
- The preview pane's header now names the label and spells out how many tasks a
  note has left.

## v0.9.0

*Released 2026-08-25*

### Added

- `scriv note` reads a directory of Markdown files — an Obsidian vault, or any
  tree of notes — as one more thing to select from. `ls` prints them,
  `sel` prints the path of the one you pick, and `edit` opens what you pick,
  several at a time on `tab`. Point `[note] root` at the vault to turn it on.
  fish binds `f10` to `note edit`.
- A note list is ordered by what you touched last, and each row carries what
  the note calls itself, the folder it is filed under and its tags, behind two
  dim columns saying how long ago it was modified and created. Titles, tags and
  creation dates are read from YAML front matter, which is also why a synced or
  freshly cloned vault still shows the dates you wrote rather than the afternoon
  the files arrived. Inline `#tags` in the body are not indexed.
- The preview pane for a note is drawn by scriv rather than by `bat`: the
  header spells out both dates and the tags, and the body arrives with its
  headings, quotes, lists, task boxes, wikilinks and tags coloured, with the
  front matter left out since the header has already said it. Nothing is
  spawned, so it keeps up with a held-down arrow key through a vault of
  thousands.
- `[note] editor` chooses what opens a note, since that is as often a reader as
  an editor — `glow` and `nvim` are both answers. Unset, it is `$VISUAL` then
  `$EDITOR`, as `scriv edit` uses. `scriv config check` reports the vault, how
  many notes are in it, and whether that editor is on `PATH`.

## v0.8.0

*Released 2026-08-17*

### Added

- A selector now says what it can do, in a line under the prompt, and can do
  more than one thing. In any pull request list, f2 opens the highlighted one
  in the browser and f7 checks it out — the same keys that do those things from
  the prompt — so the verb you meant is a key rather than another command and
  another search. In a repository list, f1 opens it on GitHub.
- ctrl-v hides and shows the preview pane in every selector that has one, which
  is what a row too wide to read wants.
- The repositories and files you actually work in are offered first.
  `repo sel`, `repo open`, `file sel` and `edit --tracked` now lead with what
  you have chosen before — frequently and recently both count — instead of
  making you type your way past a hundred and ninety-five repositories to reach
  the five you are living in. Everything unchosen keeps its old order below.
  `[selector] recent = false` turns it off and stops the choices being
  recorded; `ls` is unaffected, so anything piping it sees no change. The
  record lives in `recent`, beside your config.
- `scriv worktree add` creates a working tree and picks where it goes:
  `.worktrees/<branch>` inside the repository, or wherever `[worktree] root`
  says. A branch that does not exist yet is created, a remote-only one arrives
  tracking its remote, and the path is printed — `cd (scriv worktree add
  feat/x)` lands in the new tree. A root inside the repository is added to that
  clone's `.git/info/exclude`, so nothing else offers the tree twice.
- `scriv worktree rm` removes trees, several at a time. Neither the main tree
  nor the one you are standing in is offered.
- `scriv branch rm` deletes local branches, several at a time, each listed with
  whether git can see its commits have landed. Answering the question is what
  lets an unmerged branch go — a repository that squashes its merges has no
  other kind. Remote branches are never offered: deleting one is a push.
- `scriv proc --port 3000` narrows `ls`, `sel` and `kill` to what is listening
  on a TCP port, so "what is holding 3000" no longer means reading a pid out of
  `lsof`. `kill --port` opens no selector — a port names its processes as
  precisely as a pid does — and prints what it found as it signals them.

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
