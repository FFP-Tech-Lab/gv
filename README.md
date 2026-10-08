# gv

`gv` 是一个 Go 版本管理器，面向 Linux 与 macOS（amd64 / arm64）。它把官方 Go SDK 安装到 `~/.gv/versions`，并通过位于 `PATH` 前面的 shim 启动对应版本的 `go` 和 `gofmt`。shim 会执行真实的 Go 二进制，由 Go 自己推断 `GOROOT`。

项目目录中的 `.go-version` 优先于全局版本。本工具不修改 shell 配置文件。

## 安装

```bash
cargo install --path .
```

也可以本地构建：

```bash
cargo build --release
```

运行任意 `gv` 子命令时，会在数据目录中准备这些路径：

```text
~/.gv/
  bin/gv
  shims/go
  shims/gofmt
  versions/<version>/go/
  version
  cache/
```

`shims/go` 和 `shims/gofmt` 是 `gv` 的符号链接。程序名是 `go` 或 `gofmt` 时，`gv` 进入 shim：先解析版本，再 `exec` `$GV_ROOT/versions/<version>/go/bin/` 下的真实二进制。执行前会去掉已有的 `GOROOT`。只有在 `GOTOOLCHAIN` 未设置时，才把它设为 `local`。

## 让 shell 使用 gv

`gv init` 只打印片段，不会改 `~/.bashrc` 或 `~/.zshrc`。需要时自己加进去，或在当前 shell 中执行：

```bash
eval "$(gv init bash)"
eval "$(gv init zsh)"
```

打印出的片段是：

```bash
export GV_ROOT="${GV_ROOT:-$HOME/.gv}"
export PATH="$GV_ROOT/shims:$GV_ROOT/bin:$PATH"
```

`shims` 在 `bin` 前面。其他 shell 会报错。

## 版本如何生效

命中即停：

1. 环境变量 `GV_VERSION`
2. 从当前目录向上查找 `.go-version`
3. 全局文件 `$GV_ROOT/version`（默认 `~/.gv/version`）
4. 都没有：退出，并提示运行 `gv use <version>` 或 `gv use --global <version>`

`1.23.4` 和 `go1.23.4` 是同一个版本。只写 `1.23` 时，在已安装版本里选择最高的补丁号（按数字比较，所以 `1.23.10` 高于 `1.23.9`）。该版本尚未安装时，提示运行 `gv install <version>`。

`gv use 1.23.4` 把版本写到当前目录的 `.go-version`。`gv use --global 1.23.4` 写到全局文件。

## 命令

| 命令 | 作用 |
| --- | --- |
| `gv install <version>` | 查索引、下载、校验 SHA256、解压。已安装则直接说明。终端中显示下载进度；`--quiet` 或非终端只在结束时打印一行已传输字节数 |
| `gv uninstall <version>` | 删除该 SDK。全局文件正指向它时拒绝，除非 `--force`（同时清掉全局文件） |
| `gv list` | 列出已安装版本，并用 `*` 标出当前解析结果 |
| `gv list-remote` | 列出 stable 版本。`--all` 含 beta、rc 等历史版本。`--refresh` 强制刷新索引 |
| `gv use <version>` | 写入当前目录的 `.go-version` |
| `gv use --global <version>` | 写入全局版本文件 |
| `gv current` | 打印生效版本和来源 |
| `gv init bash` / `gv init zsh` | 打印可 `eval` 的初始化片段 |

## 环境变量

| 变量 | 含义 |
| --- | --- |
| `GV_ROOT` | 数据目录，默认 `~/.gv` |
| `GV_VERSION` | 覆盖项目文件和全局版本 |
| `GV_MIRROR` | 压缩包 URL 前缀，默认 `https://go.dev/dl`。例如 `https://mirrors.aliyun.com/golang` |
| `GV_INDEX_URL` | 版本索引，默认 `https://go.dev/dl/?mode=json&include=all` |

索引缓存在 `$GV_ROOT/cache`。校验和只使用索引 JSON 里的 `sha256`，不用压缩包旁边的校验文件。

## 下载与解压

安装包是 `.tar.gz`，顶层目录为 `go/`。文件先下到内存中校验 SHA256，再解压到临时目录。下载过程中，终端里显示已传输字节和速度。标准错误不是终端，或传入 `--quiet` 时，不绘制进度条，只在下载结束后打印一行已传输字节数。路径中出现 `..`、绝对路径，或不安全的链接目标时会拒绝，然后才把临时目录原子改名为 `versions/<version>`。最终布局是 `versions/<version>/go/bin/go`。

## 第一版不包含

- Windows
- fish、PowerShell
- 自动修改 shell 配置
- 按 `go.mod` 的 `toolchain` 行选择版本
- 调用系统里已经安装的 `go`

## 开发

```bash
cargo test
```

测试不访问网络。`rust-toolchain.toml` 指定 Rust 1.99，当前依赖需要这个编译器。程序本身使用 Rust edition 2021。

## 许可证

MIT License。Copyright 2026 FFP Tech Lab。详见 [LICENSE](LICENSE)。
