use std::collections::HashSet;
use std::path::Path;
use std::time::SystemTime;

use crate::download::{self, IndexPolicy, RemoteVersion};
use crate::error::Error;
use crate::resolve;
use crate::store;

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum OutputFormat {
    Text,
    Json,
}

pub struct ListReport {
    pub stdout: String,
    pub warning: Option<String>,
}

pub fn installed_report(
    root: &Path,
    cwd: &Path,
    gv_version: Option<&str>,
    output: OutputFormat,
) -> Result<ListReport, Error> {
    let installed = resolve::installed_versions(root)?;
    let names: Vec<String> = installed.iter().map(ToString::to_string).collect();
    let (current, warning) = match resolve::resolve_using(cwd, root, gv_version, Some(&installed)) {
        Ok(resolved) => (Some(resolved.version.to_string()), None),
        Err(Error::NoVersion) => (None, None),
        Err(err @ Error::NotInstalled(_)) => (None, Some(err.to_string())),
        Err(err) => return Err(err),
    };
    let stdout = match output {
        OutputFormat::Text => format_installed(&names, current.as_deref()),
        OutputFormat::Json => format_installed_json(&names, current.as_deref())?,
    };
    Ok(ListReport { stdout, warning })
}

pub fn format_installed(versions: &[String], current: Option<&str>) -> String {
    if versions.is_empty() {
        return "No Go versions are installed\n".to_string();
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

pub fn format_installed_json(versions: &[String], current: Option<&str>) -> Result<String, Error> {
    let installed: Vec<serde_json::Value> = versions
        .iter()
        .map(|version| {
            serde_json::json!({
                "version": version,
                "current": Some(version.as_str()) == current,
            })
        })
        .collect();
    let body = serde_json::json!({
        "current": current,
        "installed": installed,
    });
    json_line(&body)
}

pub fn format_remote_json(versions: &[RemoteVersion]) -> Result<String, Error> {
    let rows: Vec<serde_json::Value> = versions
        .iter()
        .map(|version| {
            serde_json::json!({
                "version": version.version,
                "stable": version.stable,
                "installed": version.installed,
            })
        })
        .collect();
    json_line(&serde_json::json!({ "versions": rows }))
}

fn json_line(value: &serde_json::Value) -> Result<String, Error> {
    let mut text = serde_json::to_string(value).map_err(|err| Error::Failed(err.to_string()))?;
    text.push('\n');
    Ok(text)
}

pub struct RemoteRows {
    pub rows: Vec<RemoteVersion>,
    pub warning: Option<String>,
}

pub async fn load_remote_rows(
    root: &Path,
    all: bool,
    policy: IndexPolicy,
    prefix: Option<&str>,
) -> Result<RemoteRows, Error> {
    let url = store::index_url();
    let client = download::http_client()?;
    let ui = crate::ui::Ui::detect(false);
    let loaded =
        download::load_index_with_ui(&client, root, &url, policy, SystemTime::now(), &ui).await?;
    let installed: HashSet<String> = resolve::installed_versions(root)?
        .iter()
        .map(ToString::to_string)
        .collect();
    let mut rows = download::filter_remote_rows(&loaded.releases, all);
    if let Some(prefix) = prefix.filter(|prefix| !prefix.trim().is_empty()) {
        rows.retain(|row| download::remote_version_matches(&row.version, prefix));
    }
    for row in &mut rows {
        row.installed = installed.contains(&row.version);
    }
    Ok(RemoteRows {
        rows,
        warning: loaded.warning,
    })
}

pub async fn remote(
    root: &Path,
    all: bool,
    refresh: bool,
    offline: bool,
    prefix: Option<&str>,
    output: OutputFormat,
) -> Result<(), Error> {
    if offline && refresh {
        return Err(Error::Failed(
            "Cannot combine --offline and --refresh".into(),
        ));
    }
    let policy = if refresh {
        IndexPolicy::Refresh
    } else if offline {
        IndexPolicy::Offline
    } else {
        IndexPolicy::RefreshIfStale
    };
    let loaded = load_remote_rows(root, all, policy, prefix).await?;
    if let Some(warning) = &loaded.warning {
        eprintln!("gv: {warning}");
    }
    let text = match output {
        OutputFormat::Text => download::format_remote(&loaded.rows),
        OutputFormat::Json => format_remote_json(&loaded.rows)?,
    };
    print!("{text}");
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
        let report = installed_report(root.path(), cwd.path(), None, OutputFormat::Text).unwrap();
        assert!(report.warning.is_none());
        assert_eq!(report.stdout, "* 1.23.4\n  1.22.1\n");
    }

    #[test]
    fn empty_and_missing_current_do_not_fail_the_list() {
        let root = TempDir::new();
        let cwd = TempDir::new();
        let empty = installed_report(root.path(), cwd.path(), None, OutputFormat::Text).unwrap();
        assert_eq!(empty.stdout, "No Go versions are installed\n");
        assert!(empty.warning.is_none());

        touch_sdk(root.path(), "1.22.1");
        fs::write(cwd.path().join(".go-version"), "9.9.9\n").unwrap();
        let report = installed_report(root.path(), cwd.path(), None, OutputFormat::Text).unwrap();
        assert_eq!(report.stdout, "  1.22.1\n");
        assert!(report.warning.unwrap().contains("gv install 9.9.9"));
    }

    #[test]
    fn json_lists_installed_versions_and_the_current_one() {
        let root = TempDir::new();
        let cwd = TempDir::new();
        touch_sdk(root.path(), "1.22.1");
        touch_sdk(root.path(), "1.23.4");
        fs::write(cwd.path().join(".go-version"), "1.23.4\n").unwrap();
        let report = installed_report(root.path(), cwd.path(), None, OutputFormat::Json).unwrap();
        assert!(report.warning.is_none());
        let value: serde_json::Value = serde_json::from_str(report.stdout.trim()).unwrap();
        assert_eq!(value["current"], "1.23.4");
        assert_eq!(value["installed"][0]["version"], "1.23.4");
        assert_eq!(value["installed"][0]["current"], true);
        assert_eq!(value["installed"][1]["version"], "1.22.1");
        assert_eq!(value["installed"][1]["current"], false);

        let text = installed_report(root.path(), cwd.path(), None, OutputFormat::Text).unwrap();
        assert_eq!(text.stdout, "* 1.23.4\n  1.22.1\n");

        fs::write(cwd.path().join(".go-version"), "9.9.9\n").unwrap();
        let missing = installed_report(root.path(), cwd.path(), None, OutputFormat::Json).unwrap();
        assert!(missing.warning.unwrap().contains("gv install 9.9.9"));
        let value: serde_json::Value = serde_json::from_str(missing.stdout.trim()).unwrap();
        assert!(value["current"].is_null());
        assert_eq!(value["installed"][0]["current"], false);
    }

    #[test]
    fn remote_json_includes_the_stable_flag() {
        let stable = download::RemoteVersion {
            version: "1.23.4".into(),
            stable: true,
            installed: true,
        };
        let preview = download::RemoteVersion {
            version: "1.24rc1".into(),
            stable: false,
            installed: false,
        };
        let text = format_remote_json(&[stable.clone(), preview]).unwrap();
        let value: serde_json::Value = serde_json::from_str(text.trim()).unwrap();
        assert_eq!(value["versions"][0]["version"], "1.23.4");
        assert_eq!(value["versions"][0]["stable"], true);
        assert_eq!(value["versions"][0]["installed"], true);
        assert_eq!(value["versions"][1]["version"], "1.24rc1");
        assert_eq!(value["versions"][1]["stable"], false);
        assert_eq!(value["versions"][1]["installed"], false);
        assert_eq!(download::format_remote(&[stable]), "* 1.23.4\n");
    }
}
