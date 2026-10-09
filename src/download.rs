use std::fs;
use std::io::{self, BufRead, BufReader, IsTerminal, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use flate2::read::GzDecoder;
use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tar::{Archive, EntryType};

use crate::error::Error;
use crate::platform::Platform;
use crate::resolve::{self, Version, VersionQuery};
use crate::store;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseFile {
    pub filename: String,
    pub os: String,
    pub arch: String,
    pub sha256: String,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    pub stable: bool,
    pub files: Vec<ReleaseFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallPlan {
    pub version: Version,
    pub filename: String,
    pub sha256: String,
    pub url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallStatus {
    Installed,
    AlreadyPresent,
}

#[derive(Debug, Clone)]
pub struct LoadedIndex {
    pub releases: Vec<Release>,
    pub from_cache: bool,
}

#[derive(Debug, Deserialize)]
struct RawRelease {
    version: String,
    #[serde(default)]
    stable: bool,
    #[serde(default)]
    files: Vec<RawFile>,
}

#[derive(Debug, Deserialize)]
struct RawFile {
    #[serde(default)]
    filename: String,
    #[serde(default)]
    os: String,
    #[serde(default)]
    arch: String,
    #[serde(default)]
    sha256: String,
    #[serde(default)]
    kind: String,
}

#[cfg(test)]
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex_encode(&Sha256::digest(bytes))
}

#[cfg(test)]
pub fn verify_sha256(bytes: &[u8], expected: &str) -> Result<(), Error> {
    compare_sha256(sha256_hex(bytes), expected)
}

pub fn verify_file_sha256(path: &Path, expected: &str) -> Result<(), Error> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buf)?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    compare_sha256(hex_encode(&hasher.finalize()), expected)
}

fn compare_sha256(actual: String, expected: &str) -> Result<(), Error> {
    let expected = expected.trim();
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(Error::Checksum {
            expected: expected.to_string(),
            actual,
        });
    }
    Ok(())
}

fn hex_encode(digest: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0xf) as usize] as char);
    }
    out
}

pub fn archive_url(mirror: &str, filename: &str) -> String {
    format!("{}/{}", mirror.trim().trim_end_matches('/'), filename)
}

pub fn parse_index(body: &str) -> Result<Vec<Release>, Error> {
    let values: Vec<serde_json::Value> =
        serde_json::from_str(body).map_err(|err| Error::IndexParse(err.to_string()))?;
    let mut releases = Vec::new();
    for value in values {
        let Ok(raw) = serde_json::from_value::<RawRelease>(value) else {
            continue;
        };
        let Ok(version) = resolve::parse_release_version(&raw.version) else {
            continue;
        };
        let files = raw
            .files
            .into_iter()
            .map(|file| ReleaseFile {
                filename: file.filename,
                os: file.os,
                arch: file.arch,
                sha256: file.sha256,
                kind: file.kind,
            })
            .collect();
        releases.push(Release {
            version,
            stable: raw.stable,
            files,
        });
    }
    Ok(releases)
}

pub fn plan_install(
    releases: &[Release],
    requested: &str,
    platform: &Platform,
    mirror: &str,
) -> Result<InstallPlan, Error> {
    let query = resolve::parse_install_spec(requested)?;
    let release = select_release(releases, &query)?;
    let file = select_archive(release, platform)?;
    Ok(InstallPlan {
        version: release.version.clone(),
        filename: file.filename.clone(),
        sha256: file.sha256.clone(),
        url: archive_url(mirror, &file.filename),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteVersion {
    pub version: String,
    pub stable: bool,
}

pub fn filter_remote_rows(releases: &[Release], all: bool) -> Vec<RemoteVersion> {
    let mut rows: Vec<(Version, bool)> = releases
        .iter()
        .filter(|release| all || release.stable)
        .map(|release| (release.version.clone(), release.stable))
        .collect();
    rows.sort_by(|left, right| right.0.cmp(&left.0));
    let mut out = Vec::new();
    for (version, stable) in rows {
        let name = version.to_string();
        if out
            .last()
            .is_some_and(|row: &RemoteVersion| row.version == name)
        {
            continue;
        }
        out.push(RemoteVersion {
            version: name,
            stable,
        });
    }
    out
}

pub fn filter_remote(releases: &[Release], all: bool) -> Vec<String> {
    filter_remote_rows(releases, all)
        .into_iter()
        .map(|row| row.version)
        .collect()
}

pub fn format_remote(versions: &[String]) -> String {
    if versions.is_empty() {
        return "No matching remote versions\n".to_string();
    }
    let mut text = versions.join("\n");
    text.push('\n');
    text
}

fn select_release<'a>(releases: &'a [Release], query: &VersionQuery) -> Result<&'a Release, Error> {
    match query {
        VersionQuery::Exact(version) => releases
            .iter()
            .find(|release| &release.version == version)
            .ok_or_else(|| Error::VersionNotInIndex(version.to_string())),
        VersionQuery::Minor { major, minor } => releases
            .iter()
            .filter(|release| {
                release.stable
                    && release.version.matches_minor(*major, *minor)
                    && release.version.is_stable_patch()
            })
            .max_by(|left, right| left.version.cmp(&right.version))
            .ok_or_else(|| Error::VersionNotInIndex(format!("{major}.{minor}"))),
        VersionQuery::Latest => releases
            .iter()
            .filter(|release| release.stable && release.version.is_stable_patch())
            .max_by(|left, right| left.version.cmp(&right.version))
            .ok_or_else(|| Error::VersionNotInIndex("latest".to_string())),
    }
}

fn select_archive<'a>(release: &'a Release, platform: &Platform) -> Result<&'a ReleaseFile, Error> {
    let matches: Vec<&ReleaseFile> = release
        .files
        .iter()
        .filter(|file| {
            file.kind == "archive" && file.os == platform.os && file.arch == platform.arch
        })
        .collect();
    if let Some(file) = matches
        .iter()
        .copied()
        .find(|file| filename_is_safe(&file.filename) && !file.sha256.trim().is_empty())
    {
        return Ok(file);
    }
    if let Some(file) = matches
        .iter()
        .find(|file| !filename_is_safe(&file.filename))
    {
        return Err(Error::UnsafeFilename(file.filename.clone()));
    }
    Err(Error::NoArchive {
        version: release.version.to_string(),
        os: platform.os.clone(),
        arch: platform.arch.clone(),
    })
}

fn filename_is_safe(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains("..")
        && !name.starts_with('.')
        && name.ends_with(".tar.gz")
}

pub fn read_cached_index(root: &Path, url: &str) -> Result<Option<String>, Error> {
    let dir = store::cache_dir(root);
    let url_path = dir.join("index.url");
    let body_path = dir.join("index.json");
    if !url_path.is_file() || !body_path.is_file() {
        return Ok(None);
    }
    let cached_url = fs::read_to_string(url_path)?;
    if cached_url.trim() != url.trim() {
        return Ok(None);
    }
    let body = fs::read_to_string(body_path)?;
    if body.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(body))
}

pub fn write_cached_index(root: &Path, url: &str, body: &str) -> Result<(), Error> {
    let dir = store::cache_dir(root);
    fs::create_dir_all(&dir)?;
    let body_path = dir.join("index.json");
    let url_path = dir.join("index.url");
    let tmp = dir.join(".index.json.tmp");
    fs::write(&tmp, body)?;
    fs::rename(&tmp, &body_path)?;
    fs::write(url_path, format!("{}\n", url.trim()))?;
    Ok(())
}

pub fn try_cached_index(
    root: &Path,
    url: &str,
    refresh: bool,
) -> Result<Option<Vec<Release>>, Error> {
    if refresh {
        return Ok(None);
    }
    let Some(body) = read_cached_index(root, url)? else {
        return Ok(None);
    };
    match parse_index(&body) {
        Ok(releases) => Ok(Some(releases)),
        Err(_) => Ok(None),
    }
}

pub async fn load_index(root: &Path, url: &str, refresh: bool) -> Result<LoadedIndex, Error> {
    if let Some(releases) = try_cached_index(root, url, refresh)? {
        return Ok(LoadedIndex {
            releases,
            from_cache: true,
        });
    }
    let bytes = http_get(url).await?;
    let body =
        String::from_utf8(bytes).map_err(|_| Error::IndexParse("index is not UTF-8".into()))?;
    let releases = parse_index(&body)?;
    write_cached_index(root, url, &body)?;
    Ok(LoadedIndex {
        releases,
        from_cache: false,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadUi {
    Bar,
    Plain,
}

fn download_ui(quiet: bool, stderr_is_tty: bool, term: Option<&str>) -> DownloadUi {
    let dumb = term.is_some_and(|value| value == "dumb");
    if quiet || !stderr_is_tty || dumb {
        DownloadUi::Plain
    } else {
        DownloadUi::Bar
    }
}

pub fn download_ui_for(quiet: bool) -> DownloadUi {
    let term = std::env::var("TERM").ok();
    download_ui(quiet, io::stderr().is_terminal(), term.as_deref())
}

fn format_transferred(
    label: &str,
    downloaded: u64,
    total: Option<u64>,
    elapsed: Duration,
) -> String {
    let amount = match total {
        Some(total) => format!("{downloaded}/{total}"),
        None => downloaded.to_string(),
    };
    let secs = elapsed.as_secs_f64();
    if downloaded > 0 && secs > 0.0 {
        let per_sec = (downloaded as f64 / secs).round() as u64;
        format!("Transferred {label} {amount} bytes ({per_sec} bytes/sec)\n")
    } else {
        format!("Transferred {label} {amount} bytes\n")
    }
}

pub async fn http_get(url: &str) -> Result<Vec<u8>, Error> {
    let response = send_get(url).await?;
    read_chunks(url, response, |_, _| {}).await
}

#[cfg(test)]
pub async fn download_archive(url: &str, label: &str, ui: DownloadUi) -> Result<Vec<u8>, Error> {
    let response = send_get(url).await?;
    let total = response.content_length();
    let mut progress = TransferProgress::start(ui, label, total);
    let bytes = read_chunks(url, response, |downloaded, _| {
        progress.set_downloaded(downloaded);
    })
    .await?;
    progress.finish_ok();
    Ok(bytes)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProgressRender {
    SingleBar,
    Multi,
    Plain,
}

fn progress_render(count: usize, ui: DownloadUi, stderr_hidden: bool) -> ProgressRender {
    if ui != DownloadUi::Bar || stderr_hidden {
        ProgressRender::Plain
    } else if count > 1 {
        ProgressRender::Multi
    } else {
        ProgressRender::SingleBar
    }
}

/// Download and install several plans. When there is more than one and the terminal can draw, each version gets its own progress bar; otherwise each prints one line of transferred bytes.
/// A failure for one version does not cancel the others. Archives land in `cache_dir` first and are extracted only after the checksum passes.
pub async fn install_plans(
    plans: Vec<InstallPlan>,
    ui: DownloadUi,
    versions_dir: &Path,
    cache_dir: &Path,
) -> Vec<Result<(String, InstallStatus), (String, Error)>> {
    if plans.len() <= 1 {
        return install_plans_sequential(plans, ui, versions_dir, cache_dir).await;
    }
    let render = progress_render(plans.len(), ui, ProgressDrawTarget::stderr().is_hidden());
    match render {
        ProgressRender::Multi => {
            let multi = MultiProgress::with_draw_target(ProgressDrawTarget::stderr());
            install_plans_concurrent(plans, Some(multi), versions_dir, cache_dir).await
        }
        ProgressRender::Plain | ProgressRender::SingleBar => {
            install_plans_concurrent(plans, None, versions_dir, cache_dir).await
        }
    }
}

async fn install_plans_sequential(
    plans: Vec<InstallPlan>,
    ui: DownloadUi,
    versions_dir: &Path,
    cache_dir: &Path,
) -> Vec<Result<(String, InstallStatus), (String, Error)>> {
    let mut results = Vec::with_capacity(plans.len());
    for plan in plans {
        let version = plan.version.to_string();
        match download_and_install(plan, ui, versions_dir, cache_dir).await {
            Ok(status) => results.push(Ok((version, status))),
            Err(err) => results.push(Err((version, err))),
        }
    }
    results
}

async fn install_plans_concurrent(
    plans: Vec<InstallPlan>,
    multi: Option<MultiProgress>,
    versions_dir: &Path,
    cache_dir: &Path,
) -> Vec<Result<(String, InstallStatus), (String, Error)>> {
    // The parent task holds every progress bar until all downloads finish. Dropping a
    // finished bar lets a refresh of the others clear it from the screen.
    let bars: Vec<Option<ProgressBar>> = match &multi {
        Some(multi) => plans
            .iter()
            .map(|plan| {
                let bar = multi.add(ProgressBar::no_length());
                style_bar(&bar, &plan.filename, None);
                Some(bar)
            })
            .collect(),
        None => (0..plans.len()).map(|_| None).collect(),
    };

    let mut jobs = Vec::with_capacity(plans.len());
    for (index, plan) in plans.into_iter().enumerate() {
        let versions_dir = versions_dir.to_path_buf();
        let cache_dir = cache_dir.to_path_buf();
        let bar = bars[index].clone();
        let version = plan.version.to_string();
        let handle = tokio::spawn(async move {
            if let Some(bar) = bar {
                download_and_install_bar(plan, &bar, &versions_dir, &cache_dir).await
            } else {
                download_and_install(plan, DownloadUi::Plain, &versions_dir, &cache_dir).await
            }
        });
        jobs.push((index, version, handle));
    }

    let mut finished = Vec::with_capacity(jobs.len());
    for (index, version, handle) in jobs {
        let result = match handle.await {
            Ok(Ok(status)) => Ok((version, status)),
            Ok(Err(err)) => Err((version, err)),
            Err(err) => Err((
                version,
                Error::Failed(format!("install task interrupted: {err}")),
            )),
        };
        finished.push((index, result));
    }
    drop(bars);
    finished.sort_by_key(|(index, _)| *index);
    finished.into_iter().map(|(_, result)| result).collect()
}

enum DownloadMode<'a> {
    Plain(DownloadUi),
    Bar(&'a ProgressBar),
}

async fn download_and_install(
    plan: InstallPlan,
    ui: DownloadUi,
    versions_dir: &Path,
    cache_dir: &Path,
) -> Result<InstallStatus, Error> {
    let version = plan.version.to_string();
    let path = fetch_cached_archive(&plan, cache_dir, DownloadMode::Plain(ui)).await?;
    install_archive_file(&path, versions_dir, &version)
}

async fn download_and_install_bar(
    plan: InstallPlan,
    bar: &ProgressBar,
    versions_dir: &Path,
    cache_dir: &Path,
) -> Result<InstallStatus, Error> {
    let version = plan.version.to_string();
    let path = fetch_cached_archive(&plan, cache_dir, DownloadMode::Bar(bar)).await?;
    install_archive_file(&path, versions_dir, &version)
}

fn archive_dest(cache_dir: &Path, filename: &str) -> Result<PathBuf, Error> {
    if !filename_is_safe(filename) {
        return Err(Error::UnsafeFilename(filename.to_string()));
    }
    Ok(cache_dir.join(filename))
}

/// Reuse a cached file when it exists and the checksum matches. Delete the file if the check fails; a newly downloaded file is deleted the same way if its check fails.
async fn fetch_cached_archive(
    plan: &InstallPlan,
    cache_dir: &Path,
    mode: DownloadMode<'_>,
) -> Result<PathBuf, Error> {
    let dest = archive_dest(cache_dir, &plan.filename)?;
    if dest.is_file() && verify_file_sha256(&dest, &plan.sha256).is_ok() {
        return Ok(dest);
    }
    if dest.exists() {
        fs::remove_file(&dest)?;
    }
    let downloaded = match mode {
        DownloadMode::Plain(ui) => download_archive_to(&plan.url, &plan.filename, ui, &dest).await,
        DownloadMode::Bar(bar) => {
            download_archive_bar_to(&plan.url, &plan.filename, bar, &dest).await
        }
    };
    if let Err(err) = downloaded {
        let _ = fs::remove_file(&dest);
        return Err(err);
    }
    if let Err(err) = verify_file_sha256(&dest, &plan.sha256) {
        let _ = fs::remove_file(&dest);
        return Err(err);
    }
    Ok(dest)
}

async fn download_archive_to(
    url: &str,
    label: &str,
    ui: DownloadUi,
    dest: &Path,
) -> Result<(), Error> {
    let response = send_get(url).await?;
    let total = response.content_length();
    let mut progress = TransferProgress::start(ui, label, total);
    write_response_to(url, response, dest, label, &mut progress).await?;
    progress.finish_ok();
    Ok(())
}

async fn download_archive_bar_to(
    url: &str,
    label: &str,
    bar: &ProgressBar,
    dest: &Path,
) -> Result<(), Error> {
    let response = match send_get(url).await {
        Ok(response) => response,
        Err(err) => {
            bar.finish_and_clear();
            return Err(err);
        }
    };
    let total = response.content_length();
    if let Some(len) = total {
        bar.set_length(len);
        bar.set_style(progress_style(Some(len)));
        bar.set_message(format!("Downloading {label}"));
    }
    let mut progress = TransferProgress::from_bar(label, total, bar.clone());
    write_response_to(url, response, dest, label, &mut progress).await?;
    progress.finish_ok();
    Ok(())
}

async fn write_response_to(
    url: &str,
    response: reqwest::Response,
    dest: &Path,
    label: &str,
    progress: &mut TransferProgress,
) -> Result<(), Error> {
    let parent = dest
        .parent()
        .ok_or_else(|| Error::UnsafeFilename(label.to_string()))?;
    fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(
        ".partial-{label}-{}-{}",
        std::process::id(),
        next_nonce()
    ));
    let write_result = write_response_tmp(url, response, &tmp, progress).await;
    if let Err(err) = write_result {
        let _ = fs::remove_file(&tmp);
        return Err(err);
    }
    if let Err(err) = fs::rename(&tmp, dest) {
        let _ = fs::remove_file(&tmp);
        return Err(err.into());
    }
    Ok(())
}

async fn write_response_tmp(
    url: &str,
    response: reqwest::Response,
    tmp: &Path,
    progress: &mut TransferProgress,
) -> Result<(), Error> {
    let mut file = fs::File::create(tmp)?;
    read_chunks_with(
        url,
        response,
        |chunk| {
            file.write_all(chunk)?;
            Ok(())
        },
        |downloaded, _| progress.set_downloaded(downloaded),
    )
    .await?;
    file.sync_all()?;
    Ok(())
}

fn http_err(url: &str, message: impl Into<String>) -> Error {
    Error::Http {
        url: url.to_string(),
        message: message.into(),
    }
}

async fn send_get(url: &str) -> Result<reqwest::Response, Error> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("gv/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(900))
        .build()
        .map_err(|err| http_err(url, err.to_string()))?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|err| http_err(url, err.to_string()))?;
    let status = response.status();
    if !status.is_success() {
        return Err(http_err(url, format!("HTTP {status}")));
    }
    Ok(response)
}

async fn read_chunks(
    url: &str,
    response: reqwest::Response,
    on_progress: impl FnMut(u64, Option<u64>),
) -> Result<Vec<u8>, Error> {
    let mut body = Vec::new();
    read_chunks_with(
        url,
        response,
        |chunk| {
            body.extend_from_slice(chunk);
            Ok(())
        },
        on_progress,
    )
    .await?;
    Ok(body)
}

async fn read_chunks_with(
    url: &str,
    mut response: reqwest::Response,
    mut sink: impl FnMut(&[u8]) -> Result<(), Error>,
    mut on_progress: impl FnMut(u64, Option<u64>),
) -> Result<(), Error> {
    let total = response.content_length();
    let mut downloaded = 0u64;
    loop {
        let chunk = response
            .chunk()
            .await
            .map_err(|err| http_err(url, err.to_string()))?;
        let Some(chunk) = chunk else {
            break;
        };
        sink(&chunk)?;
        downloaded += chunk.len() as u64;
        on_progress(downloaded, total);
    }
    Ok(())
}

struct TransferProgress {
    label: String,
    total: Option<u64>,
    downloaded: u64,
    started: Instant,
    bar: Option<ProgressBar>,
    finished: bool,
}

impl TransferProgress {
    fn start(ui: DownloadUi, label: &str, total: Option<u64>) -> Self {
        let bar = if ui == DownloadUi::Bar && !ProgressDrawTarget::stderr().is_hidden() {
            Some(make_bar(label, total))
        } else {
            None
        };
        Self {
            label: label.to_string(),
            total,
            downloaded: 0,
            started: Instant::now(),
            bar,
            finished: false,
        }
    }

    fn set_downloaded(&mut self, downloaded: u64) {
        self.downloaded = downloaded;
        if let Some(bar) = &self.bar {
            bar.set_position(downloaded);
        }
    }

    fn from_bar(label: &str, total: Option<u64>, bar: ProgressBar) -> Self {
        Self {
            label: label.to_string(),
            total,
            downloaded: 0,
            started: Instant::now(),
            bar: Some(bar),
            finished: false,
        }
    }

    fn finish_ok(&mut self) {
        self.finished = true;
        if let Some(bar) = &self.bar {
            bar.set_position(self.downloaded);
            bar.finish();
            return;
        }
        let line = format_transferred(
            &self.label,
            self.downloaded,
            self.total,
            self.started.elapsed(),
        );
        let mut stderr = io::stderr().lock();
        let _ = stderr.write_all(line.as_bytes());
    }
}

impl Drop for TransferProgress {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        if let Some(bar) = &self.bar {
            bar.finish_and_clear();
        }
    }
}

fn progress_style(total: Option<u64>) -> ProgressStyle {
    let template = if total.is_some() {
        "{msg} [{bar:32}] {bytes}/{total_bytes} {bytes_per_sec}"
    } else {
        "{msg} {bytes} {bytes_per_sec}"
    };
    ProgressStyle::with_template(template)
        .expect("progress bar template")
        .progress_chars("=>-")
}

fn style_bar(bar: &ProgressBar, label: &str, total: Option<u64>) {
    bar.set_style(progress_style(total));
    bar.set_message(format!("Downloading {label}"));
    bar.enable_steady_tick(Duration::from_millis(120));
}

fn make_bar(label: &str, total: Option<u64>) -> ProgressBar {
    let bar = ProgressBar::with_draw_target(total, ProgressDrawTarget::stderr());
    style_bar(&bar, label, total);
    bar
}

/// Verify the SHA256, then extract into a temporary directory. Refuse a path that contains `..` or is absolute.
#[cfg(test)]
pub fn install_verified_archive(
    bytes: &[u8],
    expected_sha256: &str,
    versions_dir: &Path,
    version: &str,
) -> Result<InstallStatus, Error> {
    verify_sha256(bytes, expected_sha256)?;
    install_tree(versions_dir, version, |tmp| extract_tarball(bytes, tmp))
}

/// Extract from a cache file that has already been verified. The caller owns the SHA256 check; do not call this if verification failed.
pub fn install_archive_file(
    path: &Path,
    versions_dir: &Path,
    version: &str,
) -> Result<InstallStatus, Error> {
    install_tree(versions_dir, version, |tmp| extract_tarball_file(path, tmp))
}

fn install_tree(
    versions_dir: &Path,
    version: &str,
    extract: impl FnOnce(&Path) -> Result<(), Error>,
) -> Result<InstallStatus, Error> {
    if !version_dir_name_is_safe(version) {
        return Err(Error::BadVersion(version.to_string()));
    }
    fs::create_dir_all(versions_dir)?;
    let final_dir = versions_dir.join(version);
    if final_dir.join("go").join("bin").join("go").is_file() {
        return Ok(InstallStatus::AlreadyPresent);
    }
    if final_dir.exists() {
        fs::remove_dir_all(&final_dir)?;
    }
    let tmp = versions_dir.join(format!(
        ".partial-{version}-{}-{}",
        std::process::id(),
        next_nonce()
    ));
    if tmp.exists() {
        fs::remove_dir_all(&tmp)?;
    }
    fs::create_dir_all(&tmp)?;
    let cleanup = RemoveAll(&tmp);
    extract(&tmp)?;
    if !tmp.join("go").join("bin").join("go").is_file() {
        return Err(Error::BadArchive("archive is missing go/bin/go".into()));
    }
    fs::rename(&tmp, &final_dir)?;
    drop(cleanup);
    Ok(InstallStatus::Installed)
}

fn version_dir_name_is_safe(version: &str) -> bool {
    !version.is_empty()
        && !version.contains('/')
        && !version.contains('\\')
        && !version.contains("..")
        && !version.starts_with('.')
}

fn next_nonce() -> u64 {
    static NONCE: AtomicU64 = AtomicU64::new(0);
    NONCE.fetch_add(1, Ordering::Relaxed)
}

struct RemoveAll<'a>(&'a Path);

impl Drop for RemoveAll<'_> {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(self.0);
    }
}

#[cfg(test)]
pub fn extract_tarball(bytes: &[u8], dest: &Path) -> Result<(), Error> {
    validate_tarball_reader(std::io::Cursor::new(bytes))?;
    extract_entries(std::io::Cursor::new(bytes), dest)
}

fn extract_tarball_file(path: &Path, dest: &Path) -> Result<(), Error> {
    validate_tarball_reader(BufReader::new(fs::File::open(path)?))?;
    extract_entries(BufReader::new(fs::File::open(path)?), dest)
}

fn extract_entries(reader: impl BufRead, dest: &Path) -> Result<(), Error> {
    fs::create_dir_all(dest)?;
    let decoder = GzDecoder::new(reader);
    let mut archive = Archive::new(decoder);
    archive.set_preserve_permissions(true);
    for entry in archive
        .entries()
        .map_err(|err| Error::BadArchive(err.to_string()))?
    {
        let mut entry = entry.map_err(|err| Error::BadArchive(err.to_string()))?;
        entry
            .unpack_in(dest)
            .map_err(|err| Error::BadArchive(err.to_string()))?;
    }
    Ok(())
}

fn validate_tarball_reader(reader: impl BufRead) -> Result<(), Error> {
    let decoder = GzDecoder::new(reader);
    let mut archive = Archive::new(decoder);
    for entry in archive
        .entries()
        .map_err(|err| Error::BadArchive(err.to_string()))?
    {
        let entry = entry.map_err(|err| Error::BadArchive(err.to_string()))?;
        let path = entry
            .path()
            .map_err(|err| Error::BadArchive(err.to_string()))?;
        check_rel_path(path.as_ref())?;
        if let Some(link) = entry
            .link_name()
            .map_err(|err| Error::BadArchive(err.to_string()))?
        {
            check_rel_path(&link)?;
        }
        if !entry_type_allowed(entry.header().entry_type()) {
            return Err(Error::UnsafePath(format!(
                "unsupported entry type: {}",
                path.display()
            )));
        }
    }
    Ok(())
}

fn check_rel_path(path: &Path) -> Result<(), Error> {
    if path.as_os_str().is_empty() {
        return Err(Error::UnsafePath("empty path".into()));
    }
    if path.is_absolute() {
        return Err(Error::UnsafePath(path.display().to_string()));
    }
    for component in path.components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(Error::UnsafePath(path.display().to_string()));
            }
        }
    }
    Ok(())
}

fn entry_type_allowed(kind: EntryType) -> bool {
    matches!(
        kind,
        EntryType::Regular
            | EntryType::Continuous
            | EntryType::Directory
            | EntryType::Symlink
            | EntryType::Link
            | EntryType::XHeader
            | EntryType::XGlobalHeader
            | EntryType::GNULongName
            | EntryType::GNULongLink
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use indicatif::{MultiProgress, ProgressDrawTarget};
    use std::fs;
    use std::io::Write;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    use tar::Builder;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn platform() -> Platform {
        Platform {
            os: "linux".into(),
            arch: "amd64".into(),
        }
    }

    fn sample_index() -> Vec<Release> {
        parse_index(
            r#"[
              {"version":"go1.23.4","stable":true,"files":[
                {"filename":"go1.23.4.src.tar.gz","os":"","arch":"","sha256":"src","kind":"source"},
                {"filename":"go1.23.4.linux-amd64.tar.gz","os":"linux","arch":"amd64","sha256":"abc","kind":"archive"},
                {"filename":"go1.23.4.darwin-arm64.tar.gz","os":"darwin","arch":"arm64","sha256":"def","kind":"archive"}
              ]},
              {"version":"go1.23.0","stable":true,"files":[
                {"filename":"go1.23.0.linux-amd64.tar.gz","os":"linux","arch":"amd64","sha256":"old","kind":"archive"}
              ]},
              {"version":"go1.23.10","stable":true,"files":[
                {"filename":"go1.23.10.linux-amd64.tar.gz","os":"linux","arch":"amd64","sha256":"new","kind":"archive"}
              ]},
              {"version":"go1.23rc1","stable":false,"files":[
                {"filename":"go1.23rc1.linux-amd64.tar.gz","os":"linux","arch":"amd64","sha256":"rc","kind":"archive"}
              ]},
              {"version":"not-a-version","stable":true,"files":[]}
            ]"#,
        )
        .unwrap()
    }

    fn gz_tar(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut builder = Builder::new(Vec::new());
        for (name, content) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_mode(0o755);
            header.set_size(content.len() as u64);
            header.set_cksum();
            builder.append_data(&mut header, *name, *content).unwrap();
        }
        let tar_bytes = builder.into_inner().unwrap();
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&tar_bytes).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn quiet_or_non_tty_uses_one_transferred_line() {
        assert_eq!(download_ui(true, true, Some("xterm")), DownloadUi::Plain);
        assert_eq!(download_ui(false, false, Some("xterm")), DownloadUi::Plain);
        assert_eq!(download_ui(false, true, Some("dumb")), DownloadUi::Plain);
        assert_eq!(download_ui(false, true, None), DownloadUi::Bar);
        assert_eq!(download_ui(false, true, Some("xterm")), DownloadUi::Bar);
        assert_eq!(
            progress_render(2, DownloadUi::Bar, false),
            ProgressRender::Multi
        );
        assert_eq!(
            progress_render(1, DownloadUi::Bar, false),
            ProgressRender::SingleBar
        );
        assert_eq!(
            progress_render(2, DownloadUi::Bar, true),
            ProgressRender::Plain
        );
        assert_eq!(
            progress_render(2, DownloadUi::Plain, false),
            ProgressRender::Plain
        );
        assert_eq!(
            format_transferred("go.tar.gz", 100, Some(200), Duration::from_secs(2)),
            "Transferred go.tar.gz 100/200 bytes (50 bytes/sec)\n"
        );
        assert_eq!(
            format_transferred("go.tar.gz", 100, None, Duration::ZERO),
            "Transferred go.tar.gz 100 bytes\n"
        );
        assert_eq!(
            format_transferred("go.tar.gz", 0, Some(0), Duration::from_secs(1)),
            "Transferred go.tar.gz 0/0 bytes\n"
        );
    }

    #[test]
    fn progress_bar_templates_accept_updates() {
        let known = make_bar("go.tar.gz", Some(4));
        known.set_position(4);
        known.finish_and_clear();
        let unknown = make_bar("go.tar.gz", None);
        unknown.set_position(4);
        unknown.finish_and_clear();
    }

    #[tokio::test]
    async fn archive_download_reports_bytes_from_local_server() {
        let body = b"hello-archive-bytes-0123456789";
        let addr = serve_once("200 OK", true, body).await;
        let url = format!("http://{addr}/go1.2.3.linux-amd64.tar.gz");
        let mut updates = Vec::new();
        let response = send_get(&url).await.unwrap();
        let bytes = tokio::time::timeout(
            Duration::from_secs(5),
            read_chunks(&url, response, |downloaded, total| {
                updates.push((downloaded, total));
            }),
        )
        .await
        .expect("download timed out")
        .unwrap();
        assert_eq!(bytes, body);
        assert!(!updates.is_empty());
        let mut previous = 0u64;
        for (downloaded, total) in &updates {
            assert!(*downloaded >= previous);
            assert_eq!(*total, Some(body.len() as u64));
            previous = *downloaded;
        }
        assert_eq!(previous, body.len() as u64);

        let addr = serve_once("200 OK", false, body).await;
        let url = format!("http://{addr}/go.tar.gz");
        let response = send_get(&url).await.unwrap();
        let mut updates = Vec::new();
        let bytes = tokio::time::timeout(
            Duration::from_secs(5),
            read_chunks(&url, response, |downloaded, total| {
                updates.push((downloaded, total));
            }),
        )
        .await
        .expect("download timed out")
        .unwrap();
        assert_eq!(bytes, body);
        assert!(updates.iter().all(|(_, total)| total.is_none()));
        assert_eq!(updates.last().map(|item| item.0), Some(body.len() as u64));

        let addr = serve_once("200 OK", true, body).await;
        let url = format!("http://{addr}/go.tar.gz");
        let archived = tokio::time::timeout(
            Duration::from_secs(5),
            download_archive(&url, "go.tar.gz", DownloadUi::Plain),
        )
        .await
        .expect("plain download timed out")
        .unwrap();
        assert_eq!(archived, body);

        let addr = serve_once("500 Internal Server Error", true, b"nope").await;
        let url = format!("http://{addr}/missing.tar.gz");
        let mut called = false;
        let err = tokio::time::timeout(Duration::from_secs(5), async {
            let response = send_get(&url).await?;
            read_chunks(&url, response, |_, _| called = true).await
        })
        .await
        .expect("status check timed out")
        .unwrap_err();
        assert!(!called);
        assert!(
            matches!(err, Error::Http { ref message, .. } if message.starts_with("HTTP 500")),
            "{err}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn installs_multiple_versions_concurrently_without_network() {
        let root = TempDir::new();
        let left = gz_tar(&[("go/bin/go", b"left-sdk")]);
        let right = gz_tar(&[("go/bin/go", b"right-sdk")]);
        let arrived = Arc::new(AtomicUsize::new(0));
        let left_addr = serve_when_all_arrived(&left, Arc::clone(&arrived), 2).await;
        let right_addr = serve_when_all_arrived(&right, Arc::clone(&arrived), 2).await;
        let plans = vec![
            plan_for(
                "1.22.5",
                &left,
                &format!("http://{left_addr}/go1.22.5.tar.gz"),
            ),
            plan_for(
                "1.23.4",
                &right,
                &format!("http://{right_addr}/go1.23.4.tar.gz"),
            ),
        ];
        let multi = MultiProgress::with_draw_target(ProgressDrawTarget::hidden());
        let versions = root.path().join("versions");
        let cache = root.path().join("archives");
        let results = tokio::time::timeout(
            Duration::from_secs(8),
            install_plans_concurrent(plans, Some(multi), &versions, &cache),
        )
        .await
        .expect("concurrent install timed out");

        assert_eq!(arrived.load(Ordering::SeqCst), 2);
        assert!(
            matches!(results[0], Ok((ref version, InstallStatus::Installed)) if version == "1.22.5"),
            "{results:?}"
        );
        assert!(
            matches!(results[1], Ok((ref version, InstallStatus::Installed)) if version == "1.23.4"),
            "{results:?}"
        );
        assert_eq!(
            fs::read(versions.join("1.22.5/go/bin/go")).unwrap(),
            b"left-sdk"
        );
        assert_eq!(
            fs::read(versions.join("1.23.4/go/bin/go")).unwrap(),
            b"right-sdk"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_install_finishes_successes_when_one_checksum_fails() {
        let root = TempDir::new();
        let good = gz_tar(&[("go/bin/go", b"good-sdk")]);
        let bad = b"not-an-archive".to_vec();
        let arrived = Arc::new(AtomicUsize::new(0));
        let good_addr = serve_when_all_arrived(&good, Arc::clone(&arrived), 2).await;
        let bad_addr = serve_when_all_arrived(&bad, Arc::clone(&arrived), 2).await;
        let plans = vec![
            plan_for(
                "1.22.5",
                b"checksum-does-not-match-body",
                &format!("http://{bad_addr}/bad.tar.gz"),
            ),
            plan_for(
                "1.23.4",
                &good,
                &format!("http://{good_addr}/go1.23.4.tar.gz"),
            ),
        ];
        let versions = root.path().join("versions");
        let cache = root.path().join("archives");
        let results = tokio::time::timeout(
            Duration::from_secs(8),
            install_plans(plans, DownloadUi::Plain, &versions, &cache),
        )
        .await
        .expect("concurrent install timed out");

        assert_eq!(arrived.load(Ordering::SeqCst), 2);
        assert!(
            matches!(results[0], Err((ref version, Error::Checksum { .. })) if version == "1.22.5"),
            "{results:?}"
        );
        assert!(
            matches!(results[1], Ok((ref version, InstallStatus::Installed)) if version == "1.23.4"),
            "{results:?}"
        );
        assert!(!versions.join("1.22.5").exists());
        assert_eq!(
            fs::read(versions.join("1.23.4/go/bin/go")).unwrap(),
            b"good-sdk"
        );
        assert!(!cache.join("go1.22.5.linux-amd64.tar.gz").exists());
        assert!(cache.join("go1.23.4.linux-amd64.tar.gz").is_file());
        let leftovers: Vec<_> = fs::read_dir(&cache)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with(".partial-"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[tokio::test]
    async fn reuses_valid_cache_without_contacting_the_server() {
        let root = TempDir::new();
        let bytes = gz_tar(&[("go/bin/go", b"cached-sdk")]);
        let plan = plan_for("1.2.3", &bytes, "http://127.0.0.1:9/go1.2.3.tar.gz");
        let cache = root.path().join("archives");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join(&plan.filename), &bytes).unwrap();
        let versions = root.path().join("versions");
        let results = tokio::time::timeout(
            Duration::from_secs(2),
            install_plans(vec![plan], DownloadUi::Plain, &versions, &cache),
        )
        .await
        .expect("cached install timed out")
        .pop()
        .unwrap();
        assert!(
            matches!(results, Ok((ref version, InstallStatus::Installed)) if version == "1.2.3"),
            "{results:?}"
        );
        assert_eq!(
            fs::read(versions.join("1.2.3/go/bin/go")).unwrap(),
            b"cached-sdk"
        );
    }

    #[tokio::test]
    async fn bad_cache_is_replaced_and_checksum_failure_deletes_the_file() {
        let root = TempDir::new();
        let bytes = gz_tar(&[("go/bin/go", b"fresh-sdk")]);
        let addr = serve_once("200 OK", true, &bytes).await;
        let plan = plan_for(
            "1.2.3",
            &bytes,
            &format!("http://{addr}/go1.2.3.linux-amd64.tar.gz"),
        );
        let cache = root.path().join("archives");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join(&plan.filename), b"corrupt-cache").unwrap();
        let versions = root.path().join("versions");
        let results = tokio::time::timeout(
            Duration::from_secs(5),
            install_plans(vec![plan.clone()], DownloadUi::Plain, &versions, &cache),
        )
        .await
        .expect("redownload timed out");
        assert!(
            matches!(results[0], Ok((ref version, InstallStatus::Installed)) if version == "1.2.3"),
            "{results:?}"
        );
        assert_eq!(fs::read(cache.join(&plan.filename)).unwrap(), bytes);
        assert_eq!(
            fs::read(versions.join("1.2.3/go/bin/go")).unwrap(),
            b"fresh-sdk"
        );
    }

    fn plan_for(version: &str, bytes: &[u8], url: &str) -> InstallPlan {
        InstallPlan {
            version: resolve::parse_release_version(&format!("go{version}")).unwrap(),
            filename: format!("go{version}.linux-amd64.tar.gz"),
            sha256: sha256_hex(bytes),
            url: url.to_string(),
        }
    }

    async fn serve_when_all_arrived(
        body: &[u8],
        arrived: Arc<AtomicUsize>,
        expected: usize,
    ) -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let body = body.to_vec();
        tokio::spawn(async move {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            if read_http_headers(&mut socket).await.is_err() {
                return;
            }
            arrived.fetch_add(1, Ordering::SeqCst);
            let deadline = tokio::time::Instant::now() + Duration::from_secs(4);
            while arrived.load(Ordering::SeqCst) < expected {
                if tokio::time::Instant::now() >= deadline {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            if socket.write_all(header.as_bytes()).await.is_err() {
                return;
            }
            let _ = socket.write_all(&body).await;
            let _ = socket.shutdown().await;
        });
        addr
    }

    async fn read_http_headers(socket: &mut tokio::net::TcpStream) -> std::io::Result<()> {
        let mut buf = Vec::new();
        let mut tmp = [0u8; 1024];
        loop {
            let n = socket.read(&mut tmp).await?;
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&tmp[..n]);
            if buf.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        Ok(())
    }

    async fn serve_once(status: &str, content_length: bool, body: &[u8]) -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let status = status.to_string();
        let body = body.to_vec();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = Vec::new();
            let mut tmp = [0u8; 1024];
            loop {
                let Ok(n) = socket.read(&mut tmp).await else {
                    return;
                };
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&tmp[..n]);
                if buf.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let mut header = format!("HTTP/1.1 {status}\r\nConnection: close\r\n");
            if content_length {
                header.push_str(&format!("Content-Length: {}\r\n", body.len()));
            }
            header.push_str("\r\n");
            if socket.write_all(header.as_bytes()).await.is_err() {
                return;
            }
            let _ = socket.write_all(&body).await;
            let _ = socket.shutdown().await;
        });
        addr
    }

    #[test]
    fn sha256_matches_known_vector_and_rejects_mismatch() {
        let expected = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert_eq!(sha256_hex(b"abc"), expected);
        verify_sha256(b"abc", &expected.to_ascii_uppercase()).unwrap();
        assert!(matches!(
            verify_sha256(b"abc", "00").unwrap_err(),
            Error::Checksum { .. }
        ));
    }

    #[test]
    fn latest_selects_highest_stable_patch_only() {
        let releases = parse_index(
            r#"[
              {"version":"go1.22.9","stable":true,"files":[
                {"filename":"go1.22.9.linux-amd64.tar.gz","os":"linux","arch":"amd64","sha256":"a","kind":"archive"}
              ]},
              {"version":"go1.23.10","stable":true,"files":[
                {"filename":"go1.23.10.linux-amd64.tar.gz","os":"linux","arch":"amd64","sha256":"b","kind":"archive"}
              ]},
              {"version":"go1.24.0","stable":true,"files":[
                {"filename":"go1.24.0.linux-amd64.tar.gz","os":"linux","arch":"amd64","sha256":"c","kind":"archive"}
              ]},
              {"version":"go1.25rc1","stable":false,"files":[
                {"filename":"go1.25rc1.linux-amd64.tar.gz","os":"linux","arch":"amd64","sha256":"d","kind":"archive"}
              ]},
              {"version":"go1.24.1rc1","stable":false,"files":[
                {"filename":"go1.24.1rc1.linux-amd64.tar.gz","os":"linux","arch":"amd64","sha256":"e","kind":"archive"}
              ]}
            ]"#,
        )
        .unwrap();
        let plan = plan_install(&releases, "latest", &platform(), "https://go.dev/dl").unwrap();
        assert_eq!(plan.version.to_string(), "1.24.0");
        assert_eq!(plan.filename, "go1.24.0.linux-amd64.tar.gz");
        let upper = plan_install(&releases, "LATEST", &platform(), "https://go.dev/dl").unwrap();
        assert_eq!(upper.version.to_string(), "1.24.0");
        let empty = parse_index(
            r#"[{"version":"go1.25rc1","stable":false,"files":[
                {"filename":"go1.25rc1.linux-amd64.tar.gz","os":"linux","arch":"amd64","sha256":"d","kind":"archive"}
            ]}]"#,
        )
        .unwrap();
        assert!(matches!(
            plan_install(&empty, "latest", &platform(), "https://go.dev/dl").unwrap_err(),
            Error::VersionNotInIndex(name) if name == "latest"
        ));
    }

    #[test]
    fn plans_highest_patch_and_joins_mirror() {
        let releases = sample_index();
        let plan = plan_install(
            &releases,
            "1.23",
            &platform(),
            "https://mirrors.aliyun.com/golang/",
        )
        .unwrap();
        assert_eq!(plan.version.to_string(), "1.23.10");
        assert_eq!(
            plan.url,
            "https://mirrors.aliyun.com/golang/go1.23.10.linux-amd64.tar.gz"
        );
        let exact = plan_install(&releases, "go1.23.4", &platform(), "https://go.dev/dl").unwrap();
        assert_eq!(exact.filename, "go1.23.4.linux-amd64.tar.gz");
        assert!(plan_install(&releases, "1.19", &platform(), "https://go.dev/dl").is_err());
    }

    #[test]
    fn rejects_unsafe_archive_filename() {
        let releases = parse_index(
            r#"[{"version":"go1.2.3","stable":true,"files":[
                {"filename":"../go1.2.3.linux-amd64.tar.gz","os":"linux","arch":"amd64","sha256":"abc","kind":"archive"}
            ]}]"#,
        )
        .unwrap();
        assert!(matches!(
            plan_install(&releases, "1.2.3", &platform(), "https://go.dev/dl").unwrap_err(),
            Error::UnsafeFilename(_)
        ));
    }

    #[test]
    fn remote_list_defaults_to_stable_and_sorts_desc() {
        let releases = sample_index();
        assert_eq!(
            filter_remote(&releases, false),
            vec!["1.23.10".to_string(), "1.23.4".into(), "1.23.0".into()]
        );
        let all = filter_remote(&releases, true);
        assert!(all.contains(&"1.23rc1".to_string()));
        assert_eq!(format_remote(&[]), "No matching remote versions\n");
        let rows = filter_remote_rows(&releases, true);
        assert!(rows
            .iter()
            .any(|row| row.version == "1.23.10" && row.stable));
        assert!(rows
            .iter()
            .any(|row| row.version == "1.23rc1" && !row.stable));
        assert!(filter_remote_rows(&releases, false)
            .iter()
            .all(|row| row.stable));
    }

    #[test]
    fn cache_respects_url_and_refresh() {
        let root = TempDir::new();
        let url = "https://example.test/index.json";
        write_cached_index(
            root.path(),
            url,
            r#"[{"version":"go1.2.3","stable":true,"files":[]}]"#,
        )
        .unwrap();
        let cached = try_cached_index(root.path(), url, false).unwrap().unwrap();
        assert_eq!(cached[0].version.to_string(), "1.2.3");
        assert!(try_cached_index(root.path(), "https://other.test", false)
            .unwrap()
            .is_none());
        assert!(try_cached_index(root.path(), url, true).unwrap().is_none());
        write_cached_index(root.path(), url, "not-json").unwrap();
        assert!(try_cached_index(root.path(), url, false).unwrap().is_none());
    }

    #[test]
    fn checksum_is_verified_before_extract() {
        let root = TempDir::new();
        let bytes = b"not-a-tarball-and-not-extracted";
        let err = install_verified_archive(bytes, "0000", &root.path().join("versions"), "1.2.3")
            .unwrap_err();
        assert!(matches!(err, Error::Checksum { .. }));
        assert!(!root.path().join("versions").exists());
    }

    #[test]
    fn rejects_parent_and_absolute_paths() {
        let root = TempDir::new();
        let bytes = raw_tar_gz(&[
            (
                b"go/../../evil.txt".as_slice(),
                b"nope".as_slice(),
                b'0',
                None,
            ),
            (b"go/bin/go".as_slice(), b"go".as_slice(), b'0', None),
        ]);
        let digest = sha256_hex(&bytes);
        let err = install_verified_archive(&bytes, &digest, &root.path().join("versions"), "1.2.3")
            .unwrap_err();
        assert!(matches!(err, Error::UnsafePath(_)), "{err}");
        assert!(!root.path().join("evil.txt").exists());
        assert!(!root.path().join("versions").join("1.2.3").exists());

        let absolute = raw_tar_gz(&[(b"/tmp/evil.txt".as_slice(), b"x".as_slice(), b'0', None)]);
        let digest = sha256_hex(&absolute);
        let err =
            install_verified_archive(&absolute, &digest, &root.path().join("versions"), "1.2.4")
                .unwrap_err();
        assert!(matches!(err, Error::UnsafePath(_)), "{err}");

        let link = raw_tar_gz(&[(
            b"go/bin/go".as_slice(),
            b"".as_slice(),
            b'2',
            Some(b"../../../evil".as_slice()),
        )]);
        let digest = sha256_hex(&link);
        let err = install_verified_archive(&link, &digest, &root.path().join("versions"), "1.2.5")
            .unwrap_err();
        assert!(matches!(err, Error::UnsafePath(_)), "{err}");
        assert!(!root.path().join("evil").exists());
    }

    #[test]
    fn installs_layout_under_version_directory() {
        let root = TempDir::new();
        let bytes = gz_tar(&[("go/bin/go", b"#!/bin/sh\n"), ("go/bin/gofmt", b"fmt")]);
        let digest = sha256_hex(&bytes);
        let status =
            install_verified_archive(&bytes, &digest, &root.path().join("versions"), "1.2.3")
                .unwrap();
        assert_eq!(status, InstallStatus::Installed);
        let go = root.path().join("versions/1.2.3/go/bin/go");
        assert_eq!(fs::read(go).unwrap(), b"#!/bin/sh\n");
        let again =
            install_verified_archive(&bytes, &digest, &root.path().join("versions"), "1.2.3")
                .unwrap();
        assert_eq!(again, InstallStatus::AlreadyPresent);
    }

    #[test]
    fn installs_from_cached_file_and_keeps_it_when_paths_are_unsafe() {
        let root = TempDir::new();
        let bytes = gz_tar(&[("go/bin/go", b"from-disk")]);
        let cache = root.path().join("archives");
        fs::create_dir_all(&cache).unwrap();
        let path = cache.join("go1.2.3.linux-amd64.tar.gz");
        fs::write(&path, &bytes).unwrap();
        let versions = root.path().join("versions");
        let status = install_archive_file(&path, &versions, "1.2.3").unwrap();
        assert_eq!(status, InstallStatus::Installed);
        assert_eq!(
            fs::read(versions.join("1.2.3/go/bin/go")).unwrap(),
            b"from-disk"
        );
        assert!(path.is_file());

        let unsafe_bytes = raw_tar_gz(&[(
            b"go/../../evil.txt".as_slice(),
            b"nope".as_slice(),
            b'0',
            None,
        )]);
        let unsafe_path = cache.join("go1.2.4.linux-amd64.tar.gz");
        fs::write(&unsafe_path, &unsafe_bytes).unwrap();
        let err = install_archive_file(&unsafe_path, &versions, "1.2.4").unwrap_err();
        assert!(matches!(err, Error::UnsafePath(_)), "{err}");
        assert!(unsafe_path.is_file());
        assert!(!versions.join("1.2.4").exists());
        assert!(!root.path().join("evil.txt").exists());
    }

    fn raw_tar_gz(files: &[(&[u8], &[u8], u8, Option<&[u8]>)]) -> Vec<u8> {
        let mut tar_bytes = Vec::new();
        for (name, content, typeflag, link) in files {
            tar_bytes.extend(ustar_header(name, content.len(), *typeflag, *link));
            tar_bytes.extend(*content);
            let pad = (512 - (content.len() % 512)) % 512;
            tar_bytes.extend(std::iter::repeat(0u8).take(pad));
        }
        tar_bytes.extend(std::iter::repeat(0u8).take(1024));
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&tar_bytes).unwrap();
        encoder.finish().unwrap()
    }

    fn ustar_header(name: &[u8], size: usize, typeflag: u8, link: Option<&[u8]>) -> [u8; 512] {
        let mut header = [0u8; 512];
        header[..name.len()].copy_from_slice(name);
        header[100..108].copy_from_slice(b"0000644\0");
        header[108..116].copy_from_slice(b"0000000\0");
        header[116..124].copy_from_slice(b"0000000\0");
        let size_field = format!("{size:011o}\0");
        header[124..136].copy_from_slice(size_field.as_bytes());
        header[136..148].copy_from_slice(b"00000000000\0");
        header[148..156].copy_from_slice(b"        ");
        header[156] = typeflag;
        if let Some(link) = link {
            header[157..157 + link.len()].copy_from_slice(link);
        }
        header[257..262].copy_from_slice(b"ustar");
        header[263..265].copy_from_slice(b"00");
        let sum: u32 = header.iter().map(|byte| u32::from(*byte)).sum();
        let checksum = format!("{sum:06o}\0 ");
        header[148..156].copy_from_slice(checksum.as_bytes());
        header
    }
}
