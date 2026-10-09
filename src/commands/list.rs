use std::cmp::Ordering;
use std::collections::HashSet;
use std::io::{self, IsTerminal};
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
                "size": version.size,
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
    let platform = crate::platform::Platform::current()?;
    let mut rows = download::filter_remote_rows(&loaded.releases, all, &platform);
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
    let expanded = prefix.is_some_and(|value| !value.trim().is_empty());
    let text = match output {
        OutputFormat::Text if io::stdout().is_terminal() => {
            let platform = crate::platform::Platform::current()?;
            let label = format!("{}/{}", platform.os, platform.arch);
            format_remote_table(&loaded.rows, expanded, &label)
        }
        OutputFormat::Text => download::format_remote(&loaded.rows),
        OutputFormat::Json => format_remote_json(&loaded.rows)?,
    };
    print!("{text}");
    Ok(())
}

struct TableLine {
    installed: bool,
    version: String,
    size: Option<u64>,
    note: String,
}

fn format_remote_table(rows: &[RemoteVersion], expanded: bool, platform_label: &str) -> String {
    if rows.is_empty() {
        return "No matching remote versions\n".to_string();
    }
    let lines = if expanded {
        expand_rows(rows)
    } else {
        collapse_rows(rows)
    };
    render_table(&lines, platform_label)
}

fn expand_rows(rows: &[RemoteVersion]) -> Vec<TableLine> {
    rows.iter()
        .map(|row| {
            let channel = if row.stable {
                None
            } else {
                preview_channel(&row.version)
            };
            TableLine::from_remote(row, note_parts(&[], 0, channel))
        })
        .collect()
}

fn collapse_rows(rows: &[RemoteVersion]) -> Vec<TableLine> {
    let mut stable_groups: Vec<Vec<&RemoteVersion>> = Vec::new();
    let mut previews = Vec::new();
    for row in rows {
        if row.stable {
            match minor_of(&row.version) {
                Some(key) => {
                    if let Some(group) = stable_groups
                        .iter_mut()
                        .find(|group| minor_of(&group[0].version) == Some(key))
                    {
                        group.push(row);
                    } else {
                        stable_groups.push(vec![row]);
                    }
                }
                None => stable_groups.push(vec![row]),
            }
        } else {
            previews.push(row);
        }
    }
    let mut lines = Vec::new();
    for mut group in stable_groups {
        group.sort_by(|left, right| cmp_version_desc(&left.version, &right.version));
        let latest = group[0];
        let have: Vec<String> = group
            .iter()
            .skip(1)
            .filter(|row| row.installed)
            .map(|row| row.version.clone())
            .collect();
        let older = group.len().saturating_sub(1);
        lines.push(TableLine::from_remote(
            latest,
            note_parts(&have, older, None),
        ));
    }
    for row in previews {
        lines.push(TableLine::from_remote(
            row,
            note_parts(&[], 0, preview_channel(&row.version)),
        ));
    }
    lines.sort_by(|left, right| cmp_version_desc(&left.version, &right.version));
    lines
}

fn note_parts(have: &[String], older: usize, channel: Option<&str>) -> String {
    let mut parts = Vec::new();
    if let Some(channel) = channel {
        parts.push(channel.to_string());
    }
    if !have.is_empty() {
        parts.push(format!("have {}", have.join(", ")));
    }
    if older > 0 {
        parts.push(format!("{older} older"));
    }
    parts.join(", ")
}

fn preview_channel(version: &str) -> Option<&'static str> {
    resolve::parse_release_version(version)
        .ok()
        .and_then(|parsed| parsed.channel())
}

fn minor_of(version: &str) -> Option<(u64, u64)> {
    resolve::parse_release_version(version)
        .ok()
        .map(|parsed| parsed.minor_key())
}

fn cmp_version_desc(left: &str, right: &str) -> Ordering {
    match (
        resolve::parse_release_version(left).ok(),
        resolve::parse_release_version(right).ok(),
    ) {
        (Some(left), Some(right)) => right.cmp(&left),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => right.cmp(left),
    }
}

impl TableLine {
    fn from_remote(row: &RemoteVersion, note: String) -> Self {
        Self {
            installed: row.installed,
            version: row.version.clone(),
            size: row.size,
            note,
        }
    }
}

fn render_table(lines: &[TableLine], platform_label: &str) -> String {
    let version_width = lines
        .iter()
        .map(|line| line.version.len())
        .max()
        .unwrap_or(0)
        .max("VERSION".len());
    let sizes: Vec<String> = lines
        .iter()
        .map(|line| match line.size {
            Some(size) => format_bytes(size),
            None => "-".to_string(),
        })
        .collect();
    let size_width = sizes
        .iter()
        .map(String::len)
        .max()
        .unwrap_or(0)
        .max("SIZE".len());
    let platform_width = platform_label.len().max("PLATFORM".len());
    let mut text = format!(
        "  {version:<version_width$}   {size:<size_width$}  {platform:<platform_width$}   NOTE\n",
        version = "VERSION",
        size = "SIZE",
        platform = "PLATFORM",
    );
    for (line, size) in lines.iter().zip(sizes) {
        let mark = if line.installed { "*" } else { " " };
        let mut row = format!(
            "{mark} {version:<version_width$}   {size:<size_width$}  {platform:<platform_width$}",
            version = line.version,
            platform = platform_label,
        );
        if !line.note.is_empty() {
            row.push_str("   ");
            row.push_str(&line.note);
        }
        row.push('\n');
        text.push_str(&row);
    }
    text
}

fn format_bytes(size: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * 1024;
    if size < KB {
        format!("{size} B")
    } else if size < MB {
        format!("{} KB", one_decimal(size, KB))
    } else {
        format!("{} MB", one_decimal(size, MB))
    }
}

fn one_decimal(size: u64, unit: u64) -> String {
    let scaled = (size as f64) / (unit as f64);
    let text = format!("{scaled:.1}");
    text.strip_suffix(".0").unwrap_or(&text).to_string()
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
            size: Some(2048),
        };
        let preview = download::RemoteVersion {
            version: "1.24rc1".into(),
            stable: false,
            installed: false,
            size: None,
        };
        let text = format_remote_json(&[stable.clone(), preview]).unwrap();
        let value: serde_json::Value = serde_json::from_str(text.trim()).unwrap();
        assert_eq!(value["versions"][0]["version"], "1.23.4");
        assert_eq!(value["versions"][0]["stable"], true);
        assert_eq!(value["versions"][0]["installed"], true);
        assert_eq!(value["versions"][0]["size"], 2048);
        assert_eq!(value["versions"][1]["version"], "1.24rc1");
        assert_eq!(value["versions"][1]["stable"], false);
        assert_eq!(value["versions"][1]["installed"], false);
        assert!(value["versions"][1]["size"].is_null());
        assert_eq!(download::format_remote(&[stable]), "* 1.23.4\n");
    }

    fn remote_row(
        version: &str,
        stable: bool,
        installed: bool,
        size: Option<u64>,
    ) -> RemoteVersion {
        RemoteVersion {
            version: version.into(),
            stable,
            installed,
            size,
        }
    }

    #[test]
    fn table_collapses_each_minor_and_notes_size_and_installed_patches() {
        let rows = vec![
            remote_row("1.24.7", true, false, Some(1024 * 1024 + 1024 * 1024 / 2)),
            remote_row("1.23.10", true, false, None),
            remote_row("1.23.4", true, true, Some(1024)),
            remote_row("1.23.0", true, false, Some(512)),
        ];
        let text = format_remote_table(&rows, false, "linux/amd64");
        assert_eq!(
            text,
            "  VERSION   SIZE    PLATFORM      NOTE\n  1.24.7    1.5 MB  linux/amd64\n  1.23.10   -       linux/amd64   have 1.23.4, 2 older\n"
        );
        assert!(!text.contains("1.23.0"));
    }

    #[test]
    fn table_expands_a_prefix_and_keeps_preview_rows_beside_collapsed_stable_lines() {
        let rows = vec![
            remote_row("1.24.0", true, false, Some(1024)),
            remote_row("1.24beta1", false, false, None),
            remote_row("1.23.4", true, true, Some(1024)),
            remote_row("1.23rc1", false, false, Some(512)),
            remote_row("1.23.0", true, false, Some(256)),
        ];
        let collapsed = format_remote_table(&rows, false, "linux/amd64");
        assert!(collapsed.contains("  1.24.0"), "{collapsed}");
        assert!(collapsed.contains("1.24beta1"), "{collapsed}");
        assert!(collapsed.contains("linux/amd64   beta"), "{collapsed}");
        assert!(collapsed.contains("* 1.23.4"), "{collapsed}");
        assert!(collapsed.contains("1 older"), "{collapsed}");
        assert!(collapsed.contains("1.23rc1"), "{collapsed}");
        assert!(collapsed.contains("linux/amd64   rc"), "{collapsed}");
        assert!(!collapsed.contains("1.23.0"), "{collapsed}");
        assert!(collapsed.contains("1 KB"), "{collapsed}");
        assert!(collapsed.contains("512 B"), "{collapsed}");

        let expanded = format_remote_table(&rows, true, "linux/amd64");
        assert!(expanded.contains("1.23.0"), "{expanded}");
        assert!(expanded.contains("1.24beta1"), "{expanded}");
        assert!(!expanded.contains("older"), "{expanded}");
        assert_eq!(
            format_remote_table(&[], false, "linux/amd64"),
            "No matching remote versions\n"
        );
    }
}
