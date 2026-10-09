<p align="center">
  <img src=".github/icon.png" alt="gv" width="240">
</p>

<h1 align="center">gv</h1>

<p align="center">
  Go version manager for Linux and macOS
</p>

<p align="center">
  <a href="README.md">English</a> |
  <a href="README.zh-CN.md">中文</a>
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-MIT-blue.svg" alt="License: MIT"></a>
  <img src="https://img.shields.io/badge/rust-1.99-orange" alt="Rust 1.99">
  <img src="https://img.shields.io/badge/platform-linux%20%7C%20macOS-lightgrey" alt="Linux and macOS">
</p>

<p align="center">
  <a href="#features">Features</a> •
  <a href="#install">Install</a> •
  <a href="#quick-start">Quick start</a> •
  <a href="#how-a-version-takes-effect">Versions</a> •
  <a href="#commands">Commands</a> •
  <a href="#development">Development</a>
</p>

`gv` installs official Go SDKs under `~/.gv/versions` and starts the matching `go` and `gofmt` through shims placed at the front of `PATH`. A shim execs the real Go binary and lets Go infer `GOROOT` itself. A `.go-version` file in the project directory takes priority over the global version.

> [!NOTE]
> `gv` does not edit shell config files. `gv init` and `gv completions` only print a snippet. Add it to your shell yourself, or `eval` it in the current session.

## Features

- Official Go SDKs for Linux and macOS (amd64 / arm64)
- `go` and `gofmt` shims that exec the selected SDK
- Project pin (`.go-version`) over the global version, with `GV_VERSION` above both
- Parallel downloads, SHA256 checks from the index, and per-version progress

## Install

```bash
cargo install --path .
```

Or build it locally:

```bash
cargo build --release
```

The first `gv` subcommand prepares this layout. `shims/go` and `shims/gofmt` are symlinks to `gv`. When the program name is `go` or `gofmt`, `gv` resolves the version and execs `$GV_ROOT/versions/<version>/go/bin/`. It removes any existing `GOROOT` before exec, and sets `GOTOOLCHAIN=local` only when `GOTOOLCHAIN` is unset.

## Quick start

Print the init snippet and load it in the current shell. `shims` comes before `bin`. Bash and zsh are supported; other shells produce an error.

```bash
eval "$(gv init bash)"
eval "$(gv init zsh)"
```

The snippet is:

```bash
export GV_ROOT="${GV_ROOT:-$HOME/.gv}"
export PATH="$GV_ROOT/shims:$GV_ROOT/bin:$PATH"
```

Install a release, pin it for this project, and confirm which binaries will run:

```bash
gv install 1.23.4
gv use 1.23.4
gv which
```

Completions are print-only as well. They do not change shell config, and they do not hook `cd`. The script completes subcommands and installed versions. `gv install` also completes `latest`.

```bash
eval "$(gv completions bash)"
eval "$(gv completions zsh)"
```

For the current shell only:

```bash
eval "$(gv shell 1.23.4)"
```

## How a version takes effect

The first match wins:

1. The `GV_VERSION` environment variable
2. A `.go-version` file found by walking up from the current directory
3. The global file `$GV_ROOT/version` (default `~/.gv/version`)
4. If none of those exist, `gv` exits and suggests `gv use <version>` or `gv use --global <version>`

`1.23.4` and `go1.23.4` are the same version. A bare `1.23` selects the highest installed patch, compared numerically, so `1.23.10` is higher than `1.23.9`. If that version is not installed yet, the tool suggests `gv install <version>`.

`gv use 1.23.4` writes `.go-version` in the current directory. `gv use --global 1.23.4` writes the global file. `gv use --unset` removes only `.go-version` in the current directory. `gv use --global --unset` removes the global file. If the file is already absent, the command says so and exits successfully. It does not walk up and remove a parent pin, and it does not install automatically when writing a pin.

`gv shell 1.23.4` prints `export GV_VERSION=1.23.4`. `gv shell --unset` prints `unset GV_VERSION`. You eval that output. The command does not register a directory-change hook. `latest` cannot be used with `gv shell` or `gv use`.

## Commands

| Command | What it does |
| --- | --- |
| `gv install <version> [version...]` | Look up the index, download, verify SHA256, and extract. Several versions passed together are downloaded at the same time. Versions that are already installed are skipped, with a note. In a terminal, each version being downloaded has its own progress bar. With `--quiet`, a non-terminal, or `TERM=dumb`, each version prints one line of transferred bytes when it finishes. If one version fails, the others still finish installing, each failure is printed, and the command exits non-zero. `latest` means the highest stable patch in the index by numeric order, and it can be combined with other versions, for example `gv install latest 1.22.5`. If it is already installed, the command skips it and exits successfully. `latest` is only for install. In a terminal, omitting the version opens an inline picker of stable remote versions. A pipe, `TERM=dumb`, and `--quiet` still require a version |
| `gv uninstall <version> [version...]` | Delete these SDKs in order. Versions that are not installed are reported one by one, and the command continues. If the global file points at one of them, that version is refused unless `--force` is set. `--force` applies to the whole command and also clears the global file. If one version fails, the others still uninstall, each failure is printed, and the command exits non-zero |
| `gv list` | List installed versions and mark the current resolution with `*`. `--output json` switches to machine-readable output that includes the installed versions and the current resolution. The default is still text |
| `gv list-remote [prefix]` | List stable versions. `--all` includes historical versions such as beta and rc. `--refresh` forces an index refresh. `--offline` uses the cache and does not download, including when the cache is stale. `--refresh` and `--offline` cannot be combined. A prefix filters the list: `1.23` keeps that minor, `1.23.4` keeps that exact version, and any other text is a prefix of the displayed version. Text marks an installed version with `*` and leaves two spaces before the others. `--output json` includes whether each version is stable and installed |
| `gv use <version>` | Write `.go-version` in the current directory. In a terminal, omitting the version opens an inline picker of installed versions. A pipe and `TERM=dumb` still require a version or `--unset` |
| `gv use --global <version>` | Write the global version file |
| `gv use --unset` | Remove `.go-version` in the current directory. If it is absent, say so and exit successfully |
| `gv use --global --unset` | Remove the global version file. If it is absent, say so and exit successfully |
| `gv current` | Print the active version and its source |
| `gv which` | Print the current resolution, its source, and the paths of `go` and `gofmt`. If the version is not installed, suggest `gv install` |
| `gv init bash` / `gv init zsh` | Print an init snippet that can be `eval`ed |
| `gv completions bash` / `gv completions zsh` | Print a completion script for subcommands, installed versions, and, for `gv install`, versions in the cached index. Completion reads that index with `gv list-remote --offline` and does not download |
| `gv clean` | Delete cached archives. It does not delete `versions/` or the index cache |
| `gv shell <version>` | Print `export GV_VERSION=<version>`. `--unset` prints `unset GV_VERSION` |

## Directory layout

```text
~/.gv/
  bin/gv
  shims/go
  shims/gofmt
  versions/<version>/go/
  version
  cache/
    index.json
    index.url
    index.fetched
    archives/
```

The final SDK layout is `versions/<version>/go/bin/go`.

## Environment variables

| Variable | Meaning |
| --- | --- |
| `GV_ROOT` | Data directory, default `~/.gv` |
| `GV_VERSION` | Overrides the project file and the global version |
| `GV_MIRROR` | Archive URL prefix, default `https://go.dev/dl`. For example `https://mirrors.aliyun.com/golang` |
| `GV_INDEX_URL` | Version index, default `https://go.dev/dl/?mode=json&include=all` |

The index is cached in `$GV_ROOT/cache`. `index.fetched` is the Unix time when that cache was written. `gv list-remote`, `gv install latest`, and a minor version such as `1.23` download a new index when the cache is missing, older than 24 hours, or has no fetch time. An exact install such as `gv install 1.23.4` keeps a parseable cache and refreshes only when that version is not in it. If a refresh fails and a cache is still readable, gv uses the cache and prints a warning. `gv list-remote --offline` never downloads. Archives are cached in `$GV_ROOT/cache/archives/`. Checksums use only the `sha256` field in the index JSON, not a checksum file next to the archive.

## Download and extract

Archives are `.tar.gz` files whose top-level directory is `go/`. Each version is written to `$GV_ROOT/cache/archives/` first. A cached file whose SHA256 matches the index is reused. After the checksum passes, it is extracted into a temporary directory. A failed check deletes that cache file.

During a download, a terminal shows transferred bytes, speed, and the estimated time remaining. When stderr is not a terminal, `TERM=dumb`, or `--quiet` is passed, no progress bar is drawn and one line of transferred bytes is printed after the download finishes. Unless `--quiet` is set, a cache hit prints `Using cached archive <filename>` and extraction prints `Extracting Go <version>`. Installing several versions downloads those archives at the same time, at most three at once: a terminal shows a progress bar for each, and a non-terminal prints one line of transferred bytes per version. A transfer that stops early keeps `cache/archives/.partial-<filename>` and retries twice, after 200ms and then 400ms. Retries cover connection failures, timeouts, and HTTP 5xx. A `206` response continues from the bytes already stored. Checksum failures, HTTP 404, and unsafe paths are not retried. When one new version is installed and it is not the version currently in effect, gv suggests `gv use <version>`.

Paths containing `..`, absolute paths, or unsafe link targets are refused. Only then is the temporary directory atomically renamed to `versions/<version>`. `gv clean` deletes only the archives in `cache/archives/`.

## Not in this version

- Windows
- fish and PowerShell
- Automatically editing shell config
- Selecting a version from the `toolchain` line in `go.mod`
- Calling a `go` binary already installed on the system

## Development

```bash
cargo test
```

Tests do not access the network. `rust-toolchain.toml` pins Rust 1.99, which the current dependencies require. The program itself uses Rust edition 2021.
