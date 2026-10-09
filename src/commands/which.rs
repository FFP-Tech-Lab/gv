use std::path::Path;

use crate::commands::current;
use crate::error::Error;
use crate::resolve;
use crate::store;

pub fn which(root: &Path, cwd: &Path, gv_version: Option<&str>) -> Result<String, Error> {
    let resolved = resolve::resolve(cwd, root, gv_version)?;
    let version = resolved.version.to_string();
    let go = store::tool_path(root, &version, "go");
    let gofmt = store::tool_path(root, &version, "gofmt");
    Ok(format!(
        "{}{}\n{}\n",
        current::format_current(&resolved),
        go.display(),
        gofmt.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{touch_sdk, TempDir};
    use std::fs;

    #[test]
    fn prints_source_and_both_tool_paths() {
        let root = TempDir::new();
        let project = TempDir::new();
        touch_sdk(root.path(), "1.23.4");
        fs::write(project.path().join(".go-version"), "go1.23.4\n").unwrap();
        let text = which(root.path(), project.path(), None).unwrap();
        let pin = project.path().join(".go-version");
        let go = store::tool_path(root.path(), "1.23.4", "go");
        let gofmt = store::tool_path(root.path(), "1.23.4", "gofmt");
        assert_eq!(
            text,
            format!(
                "1.23.4（.go-version: {}）\n{}\n{}\n",
                pin.display(),
                go.display(),
                gofmt.display()
            )
        );
    }

    #[test]
    fn missing_install_uses_the_existing_error() {
        let root = TempDir::new();
        let project = TempDir::new();
        fs::write(project.path().join(".go-version"), "1.2.3\n").unwrap();
        let err = which(root.path(), project.path(), None).unwrap_err();
        assert_eq!(err.to_string(), "未安装 Go 1.2.3。请运行 gv install 1.2.3");
    }
}
