use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::Path;
use std::process::Command;

use std::os::unix::fs::symlink;
use std::os::unix::process::CommandExt;

use crate::error::Error;
use crate::resolve;
use crate::store;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Go,
    Gofmt,
}

impl Tool {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Go => "go",
            Self::Gofmt => "gofmt",
        }
    }
}

/// Drop any existing `GOROOT` so Go infers it from the binary location. Set `GOTOOLCHAIN` to `local` only when it is unset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShimEnv {
    pub remove_goroot: bool,
    pub gotoolchain: Option<&'static str>,
}

pub fn shim_env(gotoolchain_already_set: bool) -> ShimEnv {
    ShimEnv {
        remove_goroot: true,
        gotoolchain: if gotoolchain_already_set {
            None
        } else {
            Some("local")
        },
    }
}

pub fn tool_from_argv0(arg0: &OsStr) -> Option<Tool> {
    let name = Path::new(arg0).file_name()?;
    match name.to_str()? {
        "go" => Some(Tool::Go),
        "gofmt" => Some(Tool::Gofmt),
        _ => None,
    }
}

pub fn invoked_tool() -> Option<Tool> {
    env::args_os().next().as_deref().and_then(tool_from_argv0)
}

pub fn install_shims(root: &Path) -> Result<(), Error> {
    let exe = env::current_exe()?;
    let exe = exe.canonicalize().unwrap_or(exe);
    store::ensure_layout(root)?;
    place_link(&exe, &store::bin_dir(root).join("gv"))?;
    place_link(&exe, &store::shims_dir(root).join("go"))?;
    place_link(&exe, &store::shims_dir(root).join("gofmt"))?;
    Ok(())
}

pub fn resolve_tool_binary(
    root: &Path,
    cwd: &Path,
    gv_version: Option<&str>,
    tool: Tool,
) -> Result<std::path::PathBuf, Error> {
    let resolved = resolve::resolve(cwd, root, gv_version)?;
    let version = resolved.version.to_string();
    let path = store::tool_path(root, &version, tool.as_str());
    if !path.is_file() {
        return Err(Error::MissingTool {
            version,
            tool: tool.as_str().to_string(),
        });
    }
    Ok(path)
}

pub fn execute(tool: Tool) -> Result<(), Error> {
    let root = store::gv_root()?;
    let cwd = env::current_dir()?;
    let gv_version = env::var("GV_VERSION").ok();
    let binary = resolve_tool_binary(&root, &cwd, gv_version.as_deref(), tool)?;
    let mut command = Command::new(&binary);
    command.args(env::args_os().skip(1));
    apply_shim_env(&mut command);
    let err = command.exec();
    Err(Error::Io(err))
}

fn apply_shim_env(command: &mut Command) {
    let env = shim_env(env::var_os("GOTOOLCHAIN").is_some());
    if env.remove_goroot {
        command.env_remove("GOROOT");
    }
    if let Some(value) = env.gotoolchain {
        command.env("GOTOOLCHAIN", value);
    }
}

fn place_link(exe: &Path, dest: &Path) -> Result<(), Error> {
    if dest == exe {
        return Ok(());
    }
    if let Ok(existing) = fs::read_link(dest) {
        let existing = if existing.is_absolute() {
            existing
        } else {
            dest.parent()
                .unwrap_or_else(|| Path::new("."))
                .join(existing)
        };
        if existing.canonicalize().unwrap_or(existing) == exe {
            return Ok(());
        }
    }
    if fs::symlink_metadata(dest).is_ok() {
        fs::remove_file(dest)?;
    }
    symlink(exe, dest)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{touch_sdk, TempDir};
    use std::ffi::OsStr;

    #[test]
    fn argv0_selects_only_go_and_gofmt() {
        assert_eq!(tool_from_argv0(OsStr::new("go")), Some(Tool::Go));
        assert_eq!(
            tool_from_argv0(OsStr::new("/home/a/.gv/shims/go")),
            Some(Tool::Go)
        );
        assert_eq!(tool_from_argv0(OsStr::new("gofmt")), Some(Tool::Gofmt));
        assert_eq!(
            tool_from_argv0(OsStr::new("/opt/gv/shims/gofmt")),
            Some(Tool::Gofmt)
        );
        assert_eq!(tool_from_argv0(OsStr::new("gv")), None);
        assert_eq!(tool_from_argv0(OsStr::new("go.exe")), None);
        assert_eq!(tool_from_argv0(OsStr::new("go-1.23")), None);
    }

    #[test]
    fn toolchain_env_is_set_only_when_missing() {
        assert_eq!(
            shim_env(false),
            ShimEnv {
                remove_goroot: true,
                gotoolchain: Some("local"),
            }
        );
        assert_eq!(
            shim_env(true),
            ShimEnv {
                remove_goroot: true,
                gotoolchain: None,
            }
        );
    }

    #[test]
    fn resolves_real_binary_for_selected_version() {
        let root = TempDir::new();
        let cwd = TempDir::new();
        touch_sdk(root.path(), "1.2.3");
        let go = resolve_tool_binary(root.path(), cwd.path(), Some("go1.2.3"), Tool::Go).unwrap();
        assert!(go.ends_with("versions/1.2.3/go/bin/go"));
        let gofmt =
            resolve_tool_binary(root.path(), cwd.path(), Some("1.2.3"), Tool::Gofmt).unwrap();
        assert!(gofmt.ends_with("versions/1.2.3/go/bin/gofmt"));
    }

    #[test]
    fn installs_shim_symlinks() {
        let root = TempDir::new();
        install_shims(root.path()).unwrap();
        let go = fs::read_link(root.path().join("shims/go")).unwrap();
        let gofmt = fs::read_link(root.path().join("shims/gofmt")).unwrap();
        let bin = fs::read_link(root.path().join("bin/gv")).unwrap();
        assert_eq!(go, gofmt);
        assert_eq!(go, bin);
        install_shims(root.path()).unwrap();
    }
}
