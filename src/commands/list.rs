use std::path::Path;

use crate::download;
use crate::error::Error;
use crate::resolve;
use crate::store;

pub struct ListReport {
    pub stdout: String,
    pub warning: Option<String>,
}

pub fn installed_report(
    root: &Path,
    cwd: &Path,
    gv_version: Option<&str>,
) -> Result<ListReport, Error> {
    let installed = resolve::installed_versions(root)?;
    let names: Vec<String> = installed.iter().map(ToString::to_string).collect();
    let (current, warning) = match resolve::resolve(cwd, root, gv_version) {
        Ok(resolved) => (Some(resolved.version.to_string()), None),
        Err(Error::NoVersion) => (None, None),
        Err(err @ Error::NotInstalled(_)) => (None, Some(err.to_string())),
        Err(err) => return Err(err),
    };
    Ok(ListReport {
        stdout: format_installed(&names, current.as_deref()),
        warning,
    })
}

pub fn format_installed(versions: &[String], current: Option<&str>) -> String {
    if versions.is_empty() {
        return "没有已安装的 Go 版本\n".to_string();
    }
    let mut out = String::new();
    for version in versions {
        if Some(version.as_str()) == current {
            out.push_str("* ");
        } else {
            out.push_str("  ");
        }
        out.push_str(version);
        out.push('\n');
    }
    out
}

pub async fn remote(root: &Path, all: bool, refresh: bool) -> Result<(), Error> {
    let url = store::index_url();
    let loaded = download::load_index(root, &url, refresh).await?;
    print!(
        "{}",
        download::format_remote(&download::filter_remote(&loaded.releases, all))
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{touch_sdk, TempDir};
    use std::fs;

    #[test]
    fn marks_the_resolved_version() {
        let root = TempDir::new();
        let cwd = TempDir::new();
        touch_sdk(root.path(), "1.22.1");
        touch_sdk(root.path(), "1.23.4");
        fs::write(cwd.path().join(".go-version"), "1.23.4\n").unwrap();
        let report = installed_report(root.path(), cwd.path(), None).unwrap();
        assert!(report.warning.is_none());
        assert_eq!(report.stdout, "* 1.23.4\n  1.22.1\n");
    }

    #[test]
    fn empty_and_missing_current_do_not_fail_the_list() {
        let root = TempDir::new();
        let cwd = TempDir::new();
        let empty = installed_report(root.path(), cwd.path(), None).unwrap();
        assert_eq!(empty.stdout, "没有已安装的 Go 版本\n");
        assert!(empty.warning.is_none());

        touch_sdk(root.path(), "1.22.1");
        fs::write(cwd.path().join(".go-version"), "9.9.9\n").unwrap();
        let report = installed_report(root.path(), cwd.path(), None).unwrap();
        assert_eq!(report.stdout, "  1.22.1\n");
        assert!(report.warning.unwrap().contains("gv install 9.9.9"));
    }
}
