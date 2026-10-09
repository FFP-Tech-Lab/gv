use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("No Go version is set. Run gv use <version> or gv use --global <version>")]
    NoVersion,

    #[error("Go {0} is not installed. Run gv install {0}")]
    NotInstalled(String),

    #[error("Unrecognized version: {0}")]
    BadVersion(String),

    #[error("Specify a full version, not just {0}")]
    NeedExactVersion(String),

    #[error("Version file is empty: {0}")]
    EmptyVersionFile(PathBuf),

    #[error("Version {0} is not in the index")]
    VersionNotInIndex(String),

    #[error("The index has no Go {version} archive for {os}/{arch}")]
    NoArchive {
        version: String,
        os: String,
        arch: String,
    },

    #[error("Unsafe archive filename: {0}")]
    UnsafeFilename(String),

    #[error("Unsupported platform: {0}/{1}. gv only supports Linux and macOS on amd64 and arm64")]
    UnsupportedPlatform(String, String),

    #[error("Unsupported shell: {0}. Only bash and zsh are supported")]
    UnsupportedShell(String),

    #[error("Checksum mismatch: expected {expected}, got {actual}")]
    Checksum { expected: String, actual: String },

    #[error("Refusing unsafe archive path: {0}")]
    UnsafePath(String),

    #[error("Invalid archive: {0}")]
    BadArchive(String),

    #[error("The global version points at {0}. To uninstall it, run gv uninstall {0} --force")]
    UninstallBlocked(String),

    #[error("This version is missing {tool}: {version}")]
    MissingTool { version: String, tool: String },

    #[error("HOME was not found. Set GV_ROOT")]
    NoHome,

    #[error("GV_ROOT is empty")]
    EmptyRoot,

    #[error("Cannot specify a version and --unset together")]
    VersionAndUnset,

    #[error("Specify a version, or use --unset")]
    VersionRequired,

    #[error("Failed to parse the index: {0}")]
    IndexParse(String),

    #[error("Download failed {url}: {message}")]
    Http { url: String, message: String },

    #[error("{0}")]
    Failed(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}
