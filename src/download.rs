use std::fs;
use std::io::Cursor;
use std::path::{Component, Path};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use flate2::read::GzDecoder;
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

pub fn sha256_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0xf) as usize] as char);
    }
    out
}

pub fn verify_sha256(bytes: &[u8], expected: &str) -> Result<(), Error> {
    let expected = expected.trim();
    let actual = sha256_hex(bytes);
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(Error::Checksum {
            expected: expected.to_string(),
            actual,
        });
    }
    Ok(())
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
    let query = resolve::parse_user_spec(requested)?;
    let release = select_release(releases, &query)?;
    let file = select_archive(release, platform)?;
    Ok(InstallPlan {
        version: release.version.clone(),
        filename: file.filename.clone(),
        sha256: file.sha256.clone(),
        url: archive_url(mirror, &file.filename),
    })
}

pub fn filter_remote(releases: &[Release], all: bool) -> Vec<String> {
    let mut versions: Vec<Version> = releases
        .iter()
        .filter(|release| all || release.stable)
        .map(|release| release.version.clone())
        .collect();
    versions.sort_by(|left, right| right.cmp(left));
    versions.dedup();
    versions
        .into_iter()
        .map(|version| version.to_string())
        .collect()
}

pub fn format_remote(versions: &[String]) -> String {
    if versions.is_empty() {
        return "没有匹配的远端版本\n".to_string();
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
    let body = String::from_utf8(bytes).map_err(|_| Error::IndexParse("索引不是 UTF-8".into()))?;
    let releases = parse_index(&body)?;
    write_cached_index(root, url, &body)?;
    Ok(LoadedIndex {
        releases,
        from_cache: false,
    })
}

pub async fn http_get(url: &str) -> Result<Vec<u8>, Error> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("gv/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(900))
        .build()
        .map_err(|err| Error::Http {
            url: url.to_string(),
            message: err.to_string(),
        })?;
    let response = client.get(url).send().await.map_err(|err| Error::Http {
        url: url.to_string(),
        message: err.to_string(),
    })?;
    let status = response.status();
    if !status.is_success() {
        return Err(Error::Http {
            url: url.to_string(),
            message: format!("HTTP {status}"),
        });
    }
    let bytes = response.bytes().await.map_err(|err| Error::Http {
        url: url.to_string(),
        message: err.to_string(),
    })?;
    Ok(bytes.to_vec())
}

/// 先校验 SHA256，再在临时目录解压。路径里出现 `..` 或绝对路径时拒绝写入。
pub fn install_verified_archive(
    bytes: &[u8],
    expected_sha256: &str,
    versions_dir: &Path,
    version: &str,
) -> Result<InstallStatus, Error> {
    if !version_dir_name_is_safe(version) {
        return Err(Error::BadVersion(version.to_string()));
    }
    verify_sha256(bytes, expected_sha256)?;
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
    extract_tarball(bytes, &tmp)?;
    if !tmp.join("go").join("bin").join("go").is_file() {
        return Err(Error::BadArchive("压缩包缺少 go/bin/go".into()));
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

pub fn extract_tarball(bytes: &[u8], dest: &Path) -> Result<(), Error> {
    validate_tarball(bytes)?;
    fs::create_dir_all(dest)?;
    let decoder = GzDecoder::new(Cursor::new(bytes));
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

fn validate_tarball(bytes: &[u8]) -> Result<(), Error> {
    let decoder = GzDecoder::new(Cursor::new(bytes));
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
                "不支持的条目类型：{}",
                path.display()
            )));
        }
    }
    Ok(())
}

fn check_rel_path(path: &Path) -> Result<(), Error> {
    if path.as_os_str().is_empty() {
        return Err(Error::UnsafePath("空路径".into()));
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
    use std::fs;
    use std::io::Write;
    use tar::Builder;

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
        assert_eq!(format_remote(&[]), "没有匹配的远端版本\n");
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
