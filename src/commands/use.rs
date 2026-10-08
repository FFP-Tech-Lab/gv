use std::fs;
use std::path::Path;

use crate::error::Error;
use crate::resolve;
use crate::store;

pub struct UseOutcome {
    pub message: String,
    pub hint: Option<String>,
}

pub fn run(root: &Path, cwd: &Path, requested: &str, global: bool) -> Result<UseOutcome, Error> {
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
        let outcome = run(root.path(), cwd.path(), "go1.23.4", false).unwrap();
        assert_eq!(outcome.message, "已将当前目录的 Go 版本设为 1.23.4");
        assert!(outcome.hint.is_none());
        assert_eq!(
            fs::read_to_string(cwd.path().join(".go-version")).unwrap(),
            "1.23.4\n"
        );

        let global = run(root.path(), cwd.path(), "1.22", true).unwrap();
        assert_eq!(global.message, "已将全局 Go 版本设为 1.22");
        assert!(global.hint.unwrap().contains("gv install 1.22"));
        assert_eq!(
            fs::read_to_string(root.path().join("version")).unwrap(),
            "1.22\n"
        );
    }
}
