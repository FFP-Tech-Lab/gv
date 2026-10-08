use std::path::Path;

use crate::error::Error;
use crate::resolve::{self, Origin, Resolved};

pub fn current(root: &Path, cwd: &Path, gv_version: Option<&str>) -> Result<String, Error> {
    let resolved = resolve::resolve(cwd, root, gv_version)?;
    Ok(format_current(&resolved))
}

pub fn format_current(resolved: &Resolved) -> String {
    let origin = match &resolved.origin {
        Origin::Env => "环境变量 GV_VERSION".to_string(),
        Origin::Project(path) => format!(".go-version: {}", path.display()),
        Origin::Global(path) => format!("全局: {}", path.display()),
    };
    format!("{}（{}）\n", resolved.version, origin)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::Origin;
    use crate::testutil::{touch_sdk, TempDir};
    use std::fs;

    #[test]
    fn prints_version_and_source() {
        let root = TempDir::new();
        let project = TempDir::new();
        touch_sdk(root.path(), "1.23.4");
        fs::write(project.path().join(".go-version"), "go1.23.4\n").unwrap();
        let text = current(root.path(), project.path(), None).unwrap();
        let pin = project.path().join(".go-version");
        assert_eq!(text, format!("1.23.4（.go-version: {}）\n", pin.display()));

        let from_env = resolve::resolve(project.path(), root.path(), Some("1.23.4")).unwrap();
        assert_eq!(from_env.origin, Origin::Env);
        assert_eq!(format_current(&from_env), "1.23.4（环境变量 GV_VERSION）\n");
    }
}
