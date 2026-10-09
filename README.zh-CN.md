<p align="center">
  <img src=".github/icon.png" alt="gv" width="240">
</p>

<h1 align="center">gv</h1>

<p align="center">
  适用于 Linux 与 macOS 的 Go 版本管理器
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
  <a href="#功能">功能</a> •
  <a href="#安装">安装</a> •
  <a href="#快速开始">快速开始</a> •
  <a href="#版本如何生效">版本</a> •
  <a href="#命令">命令</a> •
  <a href="#开发">开发</a>
</p>

`gv` 把官方 Go SDK 安装到 `~/.gv/versions`，并在 `PATH` 最前面放上对应的 `go` 与 `gofmt` shim。shim 会执行真正的 Go 二进制，由 Go 自己推断 `GOROOT`。项目目录里的 `.go-version` 优先于全局版本。

> [!NOTE]
> `gv` 不会修改 shell 配置文件。`gv init` 和 `gv completions` 只打印片段。你可以把它写进 shell 配置，也可以在当前会话里 `eval`。

## 功能

- 为 Linux 与 macOS（amd64 / arm64）安装官方 Go SDK
- `go` 与 `gofmt` shim 执行所选 SDK
- 项目钉选（`.go-version`）优先于全局版本，`GV_VERSION` 再优先于二者
- 并行下载，用索引里的 SHA256 校验，并按版本显示进度

## 安装

```bash
cargo install --path .
```

也可以在本地构建：

```bash
cargo build --release
```

第一次运行任意 `gv` 子命令时，会准备下面的目录。`shims/go` 和 `shims/gofmt` 是指向 `gv` 的符号链接。程序名是 `go` 或 `gofmt` 时，`gv` 会解析版本，然后执行 `$GV_ROOT/versions/<version>/go/bin/` 下的真实二进制。执行前会去掉已有的 `GOROOT`；只有在 `GOTOOLCHAIN` 未设置时，才把它设为 `local`。

## 快速开始

打印初始化片段，并在当前 shell 中加载。`shims` 排在 `bin` 前面。支持 bash 和 zsh，其他 shell 会报错。

```bash
eval "$(gv init bash)"
eval "$(gv init zsh)"
```

打印出的片段是：

```bash
export GV_ROOT="${GV_ROOT:-$HOME/.gv}"
export PATH="$GV_ROOT/shims:$GV_ROOT/bin:$PATH"
```

安装一个发行版，钉选到当前项目，并确认将要运行的二进制：

```bash
gv install 1.23.4
gv use 1.23.4
gv which
```

补全脚本同样只打印，不修改 shell 配置，也不挂钩 `cd`。脚本会补全子命令和已安装版本。`gv install` 还会补全 `latest`。

```bash
eval "$(gv completions bash)"
eval "$(gv completions zsh)"
```

只作用于当前 shell：

```bash
eval "$(gv shell 1.23.4)"
```

## 版本如何生效

按下面的顺序，命中第一个即停止：

1. 环境变量 `GV_VERSION`
2. 从当前目录向上查找得到的 `.go-version`
3. 全局文件 `$GV_ROOT/version`（默认 `~/.gv/version`）
4. 以上都不存在时，`gv` 退出，并建议运行 `gv use <version>` 或 `gv use --global <version>`

`1.23.4` 与 `go1.23.4` 是同一版本。只写 `1.23` 时，会选择已安装补丁里数值最高的一个，因此 `1.23.10` 高于 `1.23.9`。该版本尚未安装时，工具会建议运行 `gv install <version>`。

`gv use 1.23.4` 把版本写入当前目录的 `.go-version`。`gv use --global 1.23.4` 写入全局文件。`gv use --unset` 只删除当前目录的 `.go-version`。`gv use --global --unset` 删除全局文件。文件本来就不存在时，命令会说明这一点并以成功状态退出。它不会向上删除父目录的钉选，写入钉选时也不会自动安装。

`gv shell 1.23.4` 只打印 `export GV_VERSION=1.23.4`。`gv shell --unset` 打印 `unset GV_VERSION`。由你来 `eval` 这段输出。该命令不会注册目录切换钩子。`latest` 不能用于 `gv shell` 或 `gv use`。

## 命令

| 命令 | 作用 |
| --- | --- |
| `gv install <version> [version...]` | 查询索引、下载、校验 SHA256 并解压。一次传入多个版本时会同时下载。已经安装的版本会跳过，并给出说明。在终端里，每个正在下载的版本各有一条进度条。使用 `--quiet`、非终端，或 `TERM=dumb` 时，每个版本在下载结束后打印一行已传输字节数。某个版本失败时，其余版本仍会装完；每条失败都会打印，命令以非零状态退出。`latest` 表示索引中按数值排序最高的稳定补丁，可以和其他版本一起用，例如 `gv install latest 1.22.5`。若它已经安装，命令会跳过并以成功状态退出。`latest` 只用于 install。在交互终端里省略版本时，会打开一块内联选择器，列出稳定的远程版本。管道、`TERM=dumb` 和 `--quiet` 仍然必须写出版本 |
| `gv uninstall <version> [version...]` | 按顺序删除这些 SDK。未安装的版本会逐个报告，命令继续执行。若全局文件指向其中某个版本，除非加上 `--force`，否则拒绝卸载该版本。`--force` 作用于整条命令，并会清除全局文件。某个版本失败时，其余版本仍会卸载；每条失败都会打印，命令以非零状态退出 |
| `gv list` | 列出已安装版本，并用 `*` 标出当前解析结果。`--output json` 改为机器可读输出，包含已安装版本和当前解析结果。默认仍是文本 |
| `gv list-remote [前缀]` | 列出稳定版本。`--all` 包含 beta、rc 等历史版本。`--refresh` 强制刷新索引。`--offline` 只用缓存，即使缓存过期也不下载。`--refresh` 和 `--offline` 不能同时使用。前缀用来过滤：`1.23` 保留这个小版本，`1.23.4` 只保留这个精确版本，其他文本按显示出来的版本字符串做前缀匹配。文本里已安装的版本用 `*` 标出，其余版本前面是两个空格。`--output json` 会带上每个版本是否稳定、是否已安装 |
| `gv use <version>` | 在当前目录写入 `.go-version`。在交互终端里省略版本时，会打开一块内联选择器，列出已安装版本。管道和 `TERM=dumb` 仍然必须写出版本或使用 `--unset` |
| `gv use --global <version>` | 写入全局版本文件 |
| `gv use --unset` | 删除当前目录的 `.go-version`。文件不存在时说明这一点并以成功状态退出 |
| `gv use --global --unset` | 删除全局版本文件。文件不存在时说明这一点并以成功状态退出 |
| `gv current` | 打印当前生效的版本及其来源 |
| `gv which` | 打印当前解析结果、来源，以及 `go` 和 `gofmt` 的路径。版本未安装时建议运行 `gv install` |
| `gv init bash` / `gv init zsh` | 打印可供 `eval` 的初始化片段 |
| `gv completions bash` / `gv completions zsh` | 打印补全脚本，补全子命令和已安装版本。`gv install` 还会补全缓存索引里的版本。补全通过 `gv list-remote --offline` 读取索引，不会下载 |
| `gv clean` | 删除缓存的压缩包。不会删除 `versions/` 或索引缓存 |
| `gv shell <version>` | 打印 `export GV_VERSION=<version>`。`--unset` 打印 `unset GV_VERSION` |

## 目录布局

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

最终的 SDK 布局是 `versions/<version>/go/bin/go`。

## 环境变量

| 变量 | 含义 |
| --- | --- |
| `GV_ROOT` | 数据目录，默认 `~/.gv` |
| `GV_VERSION` | 覆盖项目文件和全局版本 |
| `GV_MIRROR` | 压缩包 URL 前缀，默认 `https://go.dev/dl`。例如 `https://mirrors.aliyun.com/golang` |
| `GV_INDEX_URL` | 版本索引，默认 `https://go.dev/dl/?mode=json&include=all` |

索引缓存在 `$GV_ROOT/cache`。`index.fetched` 记录这份缓存的写入时间，单位是 Unix 秒。`gv list-remote`、`gv install latest`，以及 `1.23` 这种小版本，会在缓存缺失、超过 24 小时或没有写入时间时重新下载索引。`gv install 1.23.4` 这种精确版本会继续使用能解析的缓存，只有请求的版本不在缓存里时才刷新。刷新失败但缓存仍可读时，gv 使用缓存并打印一行警告。`gv list-remote --offline` 从不下载。压缩包缓存在 `$GV_ROOT/cache/archives/`。校验和只使用索引 JSON 里的 `sha256` 字段，不使用压缩包旁边的校验文件。

## 下载与解压

压缩包是顶层目录为 `go/` 的 `.tar.gz`。每个版本先写入 `$GV_ROOT/cache/archives/`。缓存文件的 SHA256 与索引一致时会直接复用。校验通过后解压到临时目录。校验失败会删除该缓存文件。

下载时，终端会显示已传输字节、速度和预计剩余时间。终端在拉取版本索引、检查缓存压缩包（`Using cached archive <filename>`）和解压（`Extracting Go <version>`）时还会显示转圈。转圈在 80ms 后才出现第一帧。这一行随后变成 `✔`、`•` 或 `✖` 加上原来的结果句子。`✔` 为绿色，`✖` 为红色，`•` 不着色。设置 `NO_COLOR` 时仍有这些符号，只是没有颜色。`gv uninstall`、`gv use`、`gv clean` 和 `gv list-remote` 使用同一套标记。`gv list-remote` 只在下载索引时转圈，打印版本列表前清掉那一行。stderr 不是终端、`TERM=dumb`，或传入 `--quiet` 时，不画进度条也不转圈，不打印这些符号，下载结束后打印一行已传输字节数。未使用 `--quiet` 时，命中缓存会打印 `Using cached archive <filename>`，解压前会打印 `Extracting Go <version>`。一次安装多个版本时会同时下载这些压缩包，最多同时 3 个：终端为每个版本显示一条进度条，非终端则为每个版本打印一行已传输字节数。传输中断时会保留 `cache/archives/.partial-<filename>`，并再试两次，间隔 200ms 和 400ms。重试只覆盖连接失败、超时和 HTTP 5xx。服务器返回 `206` 时从已保存的字节继续。校验失败、HTTP 404 和不安全路径不会重试。只新装了一个版本、且它不是当前生效版本时，gv 会提示运行 `gv use <version>`。

包含 `..` 的路径、绝对路径或不安全的链接目标会被拒绝。通过这些检查之后，临时目录才会原子地重命名为 `versions/<version>`。`gv clean` 只删除 `cache/archives/` 里的压缩包。

## 当前版本不做的事

- Windows
- fish 与 PowerShell
- 自动修改 shell 配置
- 从 `go.mod` 的 `toolchain` 行选择版本
- 调用系统里已经安装的 `go` 二进制

## 开发

```bash
cargo test
```

测试不会访问网络。`rust-toolchain.toml` 将 Rust 固定为 1.99，当前依赖需要这个版本。程序本身使用 Rust edition 2021。
