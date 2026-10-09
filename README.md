# gv

`gv` is a Go version manager for Linux and macOS (amd64 / arm64). It installs official Go SDKs under `~/.gv/versions` and starts the matching `go` and `gofmt` through shims placed at the front of `PATH`. A shim execs the real Go binary and lets Go infer `GOROOT` itself.

A `.go-version` file in the project directory takes priority over the global version. This tool does not modify shell config files.

## Install

```bash
cargo install --path .
```

You can also build it locally:

```bash
cargo build --release
```

Running any `gv` subcommand prepares these paths in the data directory:

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
    archives/
```

`shims/go` and `shims/gofmt` are symlinks to `gv`. When the program name is `go` or `gofmt`, `gv` enters shim mode: it resolves the version, then execs the real binary under `$GV_ROOT/versions/<version>/go/bin/`. It removes any existing `GOROOT` before exec. It sets `GOTOOLCHAIN` to `local` only when `GOTOOLCHAIN` is unset.

## Use gv from the shell

`gv init` only prints a snippet. It does not change `~/.bashrc` or `~/.zshrc`. Add it yourself when you want it, or run it in the current shell:

```bash
eval "$(gv init bash)"
eval "$(gv init zsh)"
```

The printed snippet is:

```bash
export GV_ROOT="${GV_ROOT:-$HOME/.gv}"
export PATH="$GV_ROOT/shims:$GV_ROOT/bin:$PATH"
```

`shims` comes before `bin`. Other shells produce an error.

Completions are print-only as well. They do not change `~/.bashrc` or `~/.zshrc`, and they do not hook `cd`:

```bash
eval "$(gv completions bash)"
eval "$(gv completions zsh)"
```

The script completes subcommands and installed versions. `gv install` also completes `latest`.

## How a version takes effect

The first match wins:

1. The `GV_VERSION` environment variable
2. A `.go-version` file found by walking up from the current directory
3. The global file `$GV_ROOT/version` (default `~/.gv/version`)
4. If none of those exist: exit, and suggest running `gv use <version>` or `gv use --global <version>`

`1.23.4` and `go1.23.4` are the same version. A bare `1.23` selects the highest installed patch (compared numerically, so `1.23.10` is higher than `1.23.9`). If that version is not installed yet, the tool suggests running `gv install <version>`.

`gv use 1.23.4` writes the version to `.go-version` in the current directory. `gv use --global 1.23.4` writes the global file. `gv use --unset` removes only `.go-version` in the current directory; `gv use --global --unset` removes the global file. If the file is already absent, the command says so and exits successfully. It does not walk up and remove a parent pin, and it does not install automatically when writing a pin.

`gv shell 1.23.4` only prints `export GV_VERSION=1.23.4`. `gv shell --unset` prints `unset GV_VERSION`. The user evals that output. The command does not register a directory-change hook. `latest` cannot be used with `gv shell` or `gv use`.

## Commands

| Command | What it does |
| --- | --- |
| `gv install <version> [version...]` | Look up the index, download, verify SHA256, and extract. Several versions passed together are downloaded at the same time. Versions that are already installed are skipped, with a note. In a terminal, each version being downloaded has its own progress bar; with `--quiet`, a non-terminal, or `TERM=dumb`, each version prints one line of transferred bytes when it finishes. If one version fails, the others still finish installing, each failure is printed, and the command exits non-zero. `latest` means the highest stable patch in the index by numeric order, and it can be combined with other versions, for example `gv install latest 1.22.5`. If it is already installed, the command skips it and exits successfully. `latest` is only for install |
| `gv uninstall <version> [version...]` | Delete these SDKs in order. Versions that are not installed are reported one by one, and the command continues. If the global file points at one of them, that version is refused unless `--force` is set (it applies to the whole command and also clears the global file). If one version fails, the others still uninstall, each failure is printed, and the command exits non-zero |
| `gv list` | List installed versions and mark the current resolution with `*`. `--output json` switches to machine-readable output that includes the installed versions and the current resolution. The default is still the original text |
| `gv list-remote` | List stable versions. `--all` includes historical versions such as beta and rc. `--refresh` forces an index refresh. `--output json` includes whether each version is stable. Text output is unchanged |
| `gv use <version>` | Write `.go-version` in the current directory |
| `gv use --global <version>` | Write the global version file |
| `gv use --unset` | Remove `.go-version` in the current directory. If it is absent, say so and exit successfully |
| `gv use --global --unset` | Remove the global version file. If it is absent, say so and exit successfully |
| `gv current` | Print the active version and its source |
| `gv which` | Print the current resolution, its source, and the paths of `go` and `gofmt`. If the version is not installed, suggest `gv install` |
| `gv init bash` / `gv init zsh` | Print an init snippet that can be `eval`ed |
| `gv completions bash` / `gv completions zsh` | Print a completion script. It completes subcommands and installed versions |
| `gv clean` | Delete cached archives. It does not delete `versions/` or the index cache |
| `gv shell <version>` | Print `export GV_VERSION=<version>`. `--unset` prints `unset GV_VERSION` |

## Environment variables

| Variable | Meaning |
| --- | --- |
| `GV_ROOT` | Data directory, default `~/.gv` |
| `GV_VERSION` | Overrides the project file and the global version |
| `GV_MIRROR` | Archive URL prefix, default `https://go.dev/dl`. For example `https://mirrors.aliyun.com/golang` |
| `GV_INDEX_URL` | Version index, default `https://go.dev/dl/?mode=json&include=all` |

The index is cached in `$GV_ROOT/cache`. Archives are cached in `$GV_ROOT/cache/archives/`. Checksums use only the `sha256` field in the index JSON, not a checksum file next to the archive.

## Download and extract

Archives are `.tar.gz` files whose top-level directory is `go/`. Each version is written to `$GV_ROOT/cache/archives/` first. A cached file whose SHA256 matches the index is reused. After the checksum passes, it is extracted into a temporary directory; a failed check deletes that cache file. During a download, a terminal shows transferred bytes and speed. When stderr is not a terminal, `TERM=dumb`, or `--quiet` is passed, no progress bar is drawn and one line of transferred bytes is printed after the download finishes. Installing several versions downloads those archives at the same time: a terminal shows a progress bar for each, and a non-terminal prints one line of transferred bytes per version. Paths containing `..`, absolute paths, or unsafe link targets are refused, and only then is the temporary directory atomically renamed to `versions/<version>`. The final layout is `versions/<version>/go/bin/go`. `gv clean` deletes only the archives in `cache/archives/`.

## Not in the first version

- Windows
- fish, PowerShell
- Automatically editing shell config
- Selecting a version from the `toolchain` line in `go.mod`
- Calling a `go` binary already installed on the system

## Development

```bash
cargo test
```

Tests do not access the network. `rust-toolchain.toml` pins Rust 1.99, which the current dependencies require. The program itself uses Rust edition 2021.

## License

MIT License. Copyright 2026 FFP Tech Lab. See [LICENSE](LICENSE).
