use std::fs;
use std::path::Path;

use crate::error::Error;
use crate::resolve;
use crate::store;

#[derive(Debug)]
pub struct UseOutcome {
    pub message: String,
    pub hint: Option<String>,
}

pub fn run(
    root: &Path,
    cwd: &Path,
    requested: Option<&str>,
    global: bool,
) -> Result<UseOutcome, Error> {
    let Some(requested) = requested else {
        return unset(root, cwd, global);
    };
    let query = resolve::parse_user_spec(requested)?;
    let spec = query.label();
    if global {
        let path = store::global_version_path(root);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, format!("{spec}\n"))?;
    } else {
        fs::write(cwd.join(store::PIN_FILENAME), format!("{spec}\n"))?;
    }
    let installed = resolve::installed_versions(root)?;
    let present = resolve::select_installed(&query, &installed).is_ok();
    let hint = if present {
        None
    } else {
        Some(format!(
            "尚未安装该版本，运行 go 前请先执行 gv install {spec}"
        ))
    };
    let message = if global {
        format!("已将全局 Go 版本设为 {spec}")
    } else {
        format!("已将当前目录的 Go 版本设为 {spec}")
    };
    Ok(UseOutcome { message, hint })
}

fn unset(root: &Path, cwd: &Path, global: bool) -> Result<UseOutcome, Error> {
    let path = if global {
        store::global_version_path(root)
    } else {
        cwd.join(store::PIN_FILENAME)
    };
    if !path.exists() {
        let message = if global {
            "没有全局版本文件".to_string()
        } else {
            "当前目录没有 .go-version".to_string()
        };
        return Ok(UseOutcome {
            message,
            hint: None,
        });
    }
    fs::remove_file(&path)?;
    let message = if global {
        "已删除全局版本文件".to_string()
    } else {
        "已删除当前目录的 .go-version".to_string()
    };
    Ok(UseOutcome {
        message,
        hint: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{touch_sdk, TempDir};
    use std::fs;

    #[test]
    fn writes_normalized_project_and_global_pins() {
        let root = TempDir::new();
        let cwd = TempDir::new();
        touch_sdk(root.path(), "1.23.4");
        let outcome = run(root.path(), cwd.path(), Some("go1.23.4"), false).unwrap();
        assert_eq!(outcome.message, "已将当前目录的 Go 版本设为 1.23.4");
        assert!(outcome.hint.is_none());
        assert_eq!(
            fs::read_to_string(cwd.path().join(".go-version")).unwrap(),
            "1.23.4\n"
        );

        let global = run(root.path(), cwd.path(), Some("1.22"), true).unwrap();
        assert_eq!(global.message, "已将全局 Go 版本设为 1.22");
        assert!(global.hint.unwrap().contains("gv install 1.22"));
        assert_eq!(
            fs::read_to_string(root.path().join("version")).unwrap(),
            "1.22\n"
        );
    }

    #[test]
    fn unset_removes_only_the_current_directory_pin() {
        let root = TempDir::new();
        let parent = TempDir::new();
        let child = parent.path().join("child");
        fs::create_dir_all(&child).unwrap();
        fs::write(parent.path().join(".go-version"), "1.23.4\n").unwrap();
        fs::write(child.join(".go-version"), "1.22.5\n").unwrap();
        fs::write(root.path().join("version"), "1.23.4\n").unwrap();

        let other = parent.path().join("other");
        fs::create_dir_all(&other).unwrap();
        let missing = run(root.path(), &other, None, false).unwrap();
        assert_eq!(missing.message, "当前目录没有 .go-version");
        assert!(parent.path().join(".go-version").is_file());

        let removed = run(root.path(), &child, None, false).unwrap();
        assert_eq!(removed.message, "已删除当前目录的 .go-version");
        assert!(!child.join(".go-version").exists());
        assert_eq!(
            fs::read_to_string(parent.path().join(".go-version")).unwrap(),
            "1.23.4\n"
        );
        assert!(root.path().join("version").is_file());

        let again = run(root.path(), &child, None, false).unwrap();
        assert_eq!(again.message, "当前目录没有 .go-version");

        let global = run(root.path(), &child, None, true).unwrap();
        assert_eq!(global.message, "已删除全局版本文件");
        assert!(!root.path().join("version").exists());
        let global_missing = run(root.path(), &child, None, true).unwrap();
        assert_eq!(global_missing.message, "没有全局版本文件");
    }

    #[test]
    fn latest_is_not_a_use_version() {
        let root = TempDir::new();
        let cwd = TempDir::new();
        let err = run(root.path(), cwd.path(), Some("latest"), false).unwrap_err();
        assert!(err.to_string().contains("无法识别的版本号"));
        assert!(!cwd.path().join(".go-version").exists());
    }
}
