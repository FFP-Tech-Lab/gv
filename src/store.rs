use std::fs;
use std::path::{Path, PathBuf};

use crate::error::Error;

pub const DEFAULT_INDEX_URL: &str = "https://go.dev/dl/?mode=json&include=all";
pub const DEFAULT_MIRROR: &str = "https://go.dev/dl";
pub const PIN_FILENAME: &str = ".go-version";

pub fn root_from(
    gv_root: Option<&std::ffi::OsStr>,
    home: Option<&std::ffi::OsStr>,
) -> Result<PathBuf, Error> {
    if let Some(root) = gv_root {
        if root.is_empty() {
            return Err(Error::EmptyRoot);
        }
        return Ok(PathBuf::from(root));
    }
    match home {
        Some(home) if !home.is_empty() => Ok(PathBuf::from(home).join(".gv")),
        _ => Err(Error::NoHome),
    }
}

pub fn gv_root() -> Result<PathBuf, Error> {
    root_from(
        std::env::var_os("GV_ROOT").as_deref(),
        std::env::var_os("HOME").as_deref(),
    )
}

pub fn pick_config(value: Option<&str>, default: &str) -> String {
    match value {
        Some(value) if !value.trim().is_empty() => value.trim().to_string(),
        _ => default.to_string(),
    }
}

pub fn index_url() -> String {
    pick_config(
        std::env::var("GV_INDEX_URL").ok().as_deref(),
        DEFAULT_INDEX_URL,
    )
}

pub fn mirror_url() -> String {
    pick_config(std::env::var("GV_MIRROR").ok().as_deref(), DEFAULT_MIRROR)
}

pub fn versions_dir(root: &Path) -> PathBuf {
    root.join("versions")
}

pub fn version_dir(root: &Path, version: &str) -> PathBuf {
    versions_dir(root).join(version)
}

pub fn tool_path(root: &Path, version: &str, tool: &str) -> PathBuf {
    version_dir(root, version).join("go").join("bin").join(tool)
}

pub fn global_version_path(root: &Path) -> PathBuf {
    root.join("version")
}

pub fn cache_dir(root: &Path) -> PathBuf {
    root.join("cache")
}

/// Cache of official SDK archives. Index JSON stays under `cache/` and is not in this directory.
pub fn archive_cache_dir(root: &Path) -> PathBuf {
    cache_dir(root).join("archives")
}

pub fn shims_dir(root: &Path) -> PathBuf {
    root.join("shims")
}

pub fn bin_dir(root: &Path) -> PathBuf {
    root.join("bin")
}

pub fn ensure_layout(root: &Path) -> Result<(), Error> {
    fs::create_dir_all(bin_dir(root))?;
    fs::create_dir_all(shims_dir(root))?;
    fs::create_dir_all(versions_dir(root))?;
    fs::create_dir_all(cache_dir(root))?;
    Ok(())
}

pub fn read_version_text(path: &Path) -> Result<String, Error> {
    let raw = fs::read_to_string(path)?;
    let line = raw.lines().next().unwrap_or("");
    Ok(line.trim().to_string())
}

pub fn tool_exists(root: &Path, version: &str, tool: &str) -> bool {
    tool_path(root, version, tool).is_file()
}

pub fn installed_dir_names(root: &Path) -> Result<Vec<String>, Error> {
    let dir = versions_dir(root);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut names = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        if tool_path(root, &name, "go").is_file() {
            names.push(name);
        }
    }
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn root_prefers_gv_root_then_home() {
        assert_eq!(
            root_from(Some(OsStr::new("/opt/gv")), Some(OsStr::new("/home/a"))).unwrap(),
            PathBuf::from("/opt/gv")
        );
        assert_eq!(
            root_from(None, Some(OsStr::new("/home/a"))).unwrap(),
            PathBuf::from("/home/a/.gv")
        );
        assert!(matches!(root_from(None, None), Err(Error::NoHome)));
        assert!(matches!(
            root_from(Some(OsStr::new("")), Some(OsStr::new("/home/a"))),
            Err(Error::EmptyRoot)
        ));
    }

    #[test]
    fn blank_config_uses_default() {
        assert_eq!(pick_config(None, "default"), "default");
        assert_eq!(pick_config(Some("  "), "default"), "default");
        assert_eq!(
            pick_config(Some(" https://example.test/dl/ "), "default"),
            "https://example.test/dl/"
        );
    }
}
