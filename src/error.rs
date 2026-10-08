use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("未设置 Go 版本。请运行 gv use <version> 或 gv use --global <version>")]
    NoVersion,

    #[error("未安装 Go {0}。请运行 gv install {0}")]
    NotInstalled(String),

    #[error("无法识别的版本号：{0}")]
    BadVersion(String),

    #[error("请指定完整版本号，不能只写 {0}")]
    NeedExactVersion(String),

    #[error("版本文件是空的：{0}")]
    EmptyVersionFile(PathBuf),

    #[error("版本 {0} 不在索引中")]
    VersionNotInIndex(String),

    #[error("索引中没有适用于 {os}/{arch} 的 Go {version} 安装包")]
    NoArchive {
        version: String,
        os: String,
        arch: String,
    },

    #[error("安装包文件名不安全：{0}")]
    UnsafeFilename(String),

    #[error("不支持的平台：{0}/{1}。gv 仅支持 Linux 与 macOS 的 amd64 和 arm64")]
    UnsupportedPlatform(String, String),

    #[error("不支持的 shell：{0}。gv init 仅支持 bash 和 zsh")]
    UnsupportedShell(String),

    #[error("校验和不一致：期望 {expected}，实际 {actual}")]
    Checksum { expected: String, actual: String },

    #[error("拒绝不安全的压缩包路径：{0}")]
    UnsafePath(String),

    #[error("压缩包无效：{0}")]
    BadArchive(String),

    #[error("全局版本正指向 {0}。如需卸载请运行 gv uninstall {0} --force")]
    UninstallBlocked(String),

    #[error("该版本缺少 {tool}：{version}")]
    MissingTool { version: String, tool: String },

    #[error("未找到 HOME，请设置 GV_ROOT")]
    NoHome,

    #[error("GV_ROOT 为空")]
    EmptyRoot,

    #[error("索引解析失败：{0}")]
    IndexParse(String),

    #[error("下载失败 {url}：{message}")]
    Http { url: String, message: String },

    #[error(transparent)]
    Io(#[from] std::io::Error),
}
