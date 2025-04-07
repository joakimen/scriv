# scriv

> "We found him wandering around, with a candle."

## Summary

[Scriv](https://kingkiller.fandom.com/wiki/Scriv) is a CLI tool that detects Git repositories.

## Description

Scriv detects Git repositories by searching recursively in one or more directories configured by the user, and returns a list of absolute paths to the discovered repositories.

A repository is defined here as a directory that contains a .git subdirectory.

Searching is done recursively, and with a depth specified on a per-path basis.

## Install

With Go:

```sh
go install github.com/joakimen/scriv@latest
```

## Usage

List repositories

```sh
$ scriv list
/Users/joakim/dev/github.com/joakimen/scriv
/Users/joakim/dev/github.com/joakimen/fzf.clj
...
```

Print resolved configuration

```sh
$ scriv config
{
  "Paths": [
    {
      "Path": "~/dev/github.com",
      "Depth": 2
    },
    {
      "Path": "~/bin",
      "Depth": 0
    }
  ],
  "Settings": {
    "Ignore": [
      "node_modules",
      "vendor",
      "dist",
      "build",
      "target"
    ]
  },
  "Logger": {}
}
```

## Configuration

Configuration is done by specifying one or more paths, along with their desired search depth.

### Example

```json
{
  "paths": [
    { "path": "~/dev/github.com", "depth": 2 },
    { "path": "~/bin", "depth": 0 }
  ],
  "settings": {
    "ignore": ["node_modules", "target"]
  }
}
```

### Configuration keys

#### `.paths[].path`

Required.

The root path under which to search for repos. The root path may itself be a repo.

#### `.paths[].depth`

Optional.

The search depth for the associated path.

Default: 0

Tune this according to your project layout, as this is the primary determining factor for the discovery performance.

##### Examples

###### Example 1: `~/dev/github.com + depth: 2`

- `~/dev/github.com/repo1` will be returned
- `~/dev/github.com/dir1/repo1` will be returned
- `~/dev/github.com/dir1/dir2/repo1` will **not** be returned

###### Example 2: `~/bin + depth: 0`

- `~/bin` will be returned
- `~/bin/repo1` will **not** be returned

#### `.settings.ignore`

Optional.

Paths to ignore during search.
Default: `"node_modules", "vendor", "dist", "build", "target"`
