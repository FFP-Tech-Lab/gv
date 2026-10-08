use std::fs;
use std::path::Path;

use crate::error::Error;
use crate::resolve::{self, Version, VersionQuery};
use crate::store;

pub fn run(root: &Path, requested: &str, force: bool) -> Result<String, Error> {
    let version = resolve::require_exact(requested)?;
    let name = version.to_string();
    if !store::tool_exists(root, &name, "go") {
        return Err(Error::NotInstalled(name));
    }
    let points = global_points_at(root, &version)?;
    if points && !force {
        return Err(Error::UninstallBlocked(name));
    }
    let dir = store::version_dir(root, &name);
    let meta = fs::symlink_metadata(&dir)?;
    if meta.file_type().is_symlink() {
        return Err(Error::UnsafePath(dir.display().to_string()));
    }
    fs::remove_dir_all(&dir)?;
    if points {
        fs::remove_file(store::global_version_path(root))?;
        return Ok(format!("已卸载 Go {name}，并清除全局版本"));
    }
    Ok(format!("已卸载 Go {name}"))
}

fn global_points_at(root: &Path, version: &Version) -> Result<bool, Error> {
    let path = store::global_version_path(root);
    if !path.is_file() {
        return Ok(false);
    }
    let text = store::read_version_text(&path)?;
    if text.is_empty() {
        return Ok(false);
    }
    match resolve::parse_user_spec(&text) {
        Ok(VersionQuery::Exact(pinned)) => Ok(pinned == *version),
        _ => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{touch_sdk, TempDir};
    use std::fs;

    #[test]
    fn refuses_when_global_points_at_version_unless_forced() {
        let root = TempDir::new();
        touch_sdk(root.path(), "1.23.4");
        touch_sdk(root.path(), "1.22.0");
        fs::write(root.path().join("version"), "go1.23.4\n").unwrap();

        let err = run(root.path(), "1.23.4", false).unwrap_err();
        assert!(err.to_string().contains("gv uninstall 1.23.4 --force"));
        assert!(store::tool_exists(root.path(), "1.23.4", "go"));

        let message = run(root.path(), "1.22.0", false).unwrap();
        assert_eq!(message, "已卸载 Go 1.22.0");
        assert!(!store::tool_exists(root.path(), "1.22.0", "go"));
        assert!(store::tool_exists(root.path(), "1.23.4", "go"));

        let forced = run(root.path(), "1.23.4", true).unwrap();
        assert_eq!(forced, "已卸载 Go 1.23.4，并清除全局版本");
        assert!(!root.path().join("version").exists());
        assert!(!store::version_dir(root.path(), "1.23.4").exists());
    }

    #[test]
    fn minor_version_is_rejected() {
        let root = TempDir::new();
        touch_sdk(root.path(), "1.23.4");
        assert!(matches!(
            run(root.path(), "1.23", false),
            Err(Error::NeedExactVersion(_))
        ));
    }
}
