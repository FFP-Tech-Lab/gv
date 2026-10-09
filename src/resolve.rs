use std::cmp::Ordering;
use std::env;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::error::Error;
use crate::store;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum PreKind {
    Beta,
    Rc,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Pre {
    kind: PreKind,
    n: u64,
}

/// A concrete version from an install or the index. A missing patch means a prerelease such as `1.23rc1`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    major: u64,
    minor: u64,
    patch: Option<u64>,
    pre: Option<Pre>,
}

impl Version {
    pub fn is_stable_patch(&self) -> bool {
        self.patch.is_some() && self.pre.is_none()
    }

    pub fn matches_minor(&self, major: u64, minor: u64) -> bool {
        self.major == major && self.minor == minor
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)?;
        if let Some(patch) = self.patch {
            write!(f, ".{patch}")?;
        }
        if let Some(pre) = &self.pre {
            let kind = match pre.kind {
                PreKind::Beta => "beta",
                PreKind::Rc => "rc",
            };
            write!(f, "{kind}{}", pre.n)?;
        }
        Ok(())
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.major
            .cmp(&other.major)
            .then(self.minor.cmp(&other.minor))
            .then(self.patch.unwrap_or(0).cmp(&other.patch.unwrap_or(0)))
            .then_with(|| cmp_pre(self.pre.as_ref(), other.pre.as_ref()))
    }
}

fn cmp_pre(left: Option<&Pre>, right: Option<&Pre>) -> Ordering {
    match (left, right) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(left), Some(right)) => left.kind.cmp(&right.kind).then(left.n.cmp(&right.n)),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionQuery {
    Exact(Version),
    Minor {
        major: u64,
        minor: u64,
    },
    /// Only for `gv install`. It is not written to version files and does not change `gv use`.
    Latest,
}

impl VersionQuery {
    pub fn label(&self) -> String {
        match self {
            Self::Exact(version) => version.to_string(),
            Self::Minor { major, minor } => format!("{major}.{minor}"),
            Self::Latest => "latest".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    Env,
    Project(PathBuf),
    Global(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub version: Version,
    pub origin: Origin,
}

/// `latest` is valid only for install. `gv use` and version files still go through `parse_user_spec`.
pub fn parse_install_spec(input: &str) -> Result<VersionQuery, Error> {
    if input.trim().eq_ignore_ascii_case("latest") {
        return Ok(VersionQuery::Latest);
    }
    parse_user_spec(input)
}

pub fn parse_user_spec(input: &str) -> Result<VersionQuery, Error> {
    let trimmed = input.trim();
    let body = strip_go_prefix(trimmed);
    if body.is_empty() {
        return Err(Error::BadVersion(input.to_string()));
    }
    let (major, rest) = split_number(body).map_err(|_| Error::BadVersion(input.to_string()))?;
    let rest = rest
        .strip_prefix('.')
        .ok_or_else(|| Error::BadVersion(input.to_string()))?;
    let (minor, rest) = split_number(rest).map_err(|_| Error::BadVersion(input.to_string()))?;
    if rest.is_empty() {
        return Ok(VersionQuery::Minor { major, minor });
    }
    let version =
        parse_rest(major, minor, rest).map_err(|_| Error::BadVersion(input.to_string()))?;
    Ok(VersionQuery::Exact(version))
}

pub fn parse_release_version(input: &str) -> Result<Version, Error> {
    match parse_user_spec(input)? {
        VersionQuery::Exact(version) => Ok(version),
        VersionQuery::Minor { .. } | VersionQuery::Latest => {
            Err(Error::BadVersion(input.to_string()))
        }
    }
}

pub fn require_exact(input: &str) -> Result<Version, Error> {
    match parse_user_spec(input)? {
        VersionQuery::Exact(version) => Ok(version),
        VersionQuery::Minor { major, minor } => {
            Err(Error::NeedExactVersion(format!("{major}.{minor}")))
        }
        VersionQuery::Latest => Err(Error::BadVersion(input.to_string())),
    }
}

pub fn select_installed(query: &VersionQuery, installed: &[Version]) -> Result<Version, Error> {
    match query {
        VersionQuery::Exact(version) => {
            if installed.iter().any(|item| item == version) {
                Ok(version.clone())
            } else {
                Err(Error::NotInstalled(version.to_string()))
            }
        }
        VersionQuery::Minor { major, minor } => installed
            .iter()
            .filter(|version| {
                version.major == *major && version.minor == *minor && version.is_stable_patch()
            })
            .max()
            .cloned()
            .ok_or_else(|| Error::NotInstalled(format!("{major}.{minor}"))),
        VersionQuery::Latest => Err(Error::BadVersion("latest".to_string())),
    }
}

pub fn installed_versions(root: &Path) -> Result<Vec<Version>, Error> {
    let mut versions = Vec::new();
    for name in store::installed_dir_names(root)? {
        if let Ok(version) = parse_release_version(&name) {
            versions.push(version);
        }
    }
    versions.sort_by(|left, right| right.cmp(left));
    Ok(versions)
}

pub fn resolve(cwd: &Path, root: &Path, gv_version: Option<&str>) -> Result<Resolved, Error> {
    resolve_using(cwd, root, gv_version, None)
}

/// Resolve using an already-scanned install list. `gv list` passes that list so it does not scan `versions/` twice.
pub fn resolve_using(
    cwd: &Path,
    root: &Path,
    gv_version: Option<&str>,
    installed: Option<&[Version]>,
) -> Result<Resolved, Error> {
    if let Some(raw) = gv_version {
        let raw = raw.trim();
        if !raw.is_empty() {
            return activate(parse_user_spec(raw)?, Origin::Env, root, installed);
        }
    }
    if let Some((path, text)) = find_project_pin(cwd)? {
        return activate(parse_user_spec(&text)?, Origin::Project(path), root, installed);
    }
    let global = store::global_version_path(root);
    if global.is_file() {
        let text = store::read_version_text(&global)?;
        if text.is_empty() {
            return Err(Error::EmptyVersionFile(global));
        }
        return activate(parse_user_spec(&text)?, Origin::Global(global), root, installed);
    }
    Err(Error::NoVersion)
}

fn activate(
    query: VersionQuery,
    origin: Origin,
    root: &Path,
    installed: Option<&[Version]>,
) -> Result<Resolved, Error> {
    let version = match &query {
        VersionQuery::Exact(version) => {
            if let Some(installed) = installed {
                select_installed(&query, installed)?
            } else if store::tool_exists(root, &version.to_string(), "go") {
                version.clone()
            } else {
                return Err(Error::NotInstalled(version.to_string()));
            }
        }
        VersionQuery::Minor { .. } => {
            let owned;
            let installed = if let Some(installed) = installed {
                installed
            } else {
                owned = installed_versions(root)?;
                &owned
            };
            select_installed(&query, installed)?
        }
        VersionQuery::Latest => return Err(Error::BadVersion("latest".to_string())),
    };
    Ok(Resolved { version, origin })
}

fn find_project_pin(start: &Path) -> Result<Option<(PathBuf, String)>, Error> {
    let mut dir = if start.is_absolute() {
        start.to_path_buf()
    } else {
        env::current_dir()?.join(start)
    };
    loop {
        let candidate = dir.join(store::PIN_FILENAME);
        if candidate.is_file() {
            let text = store::read_version_text(&candidate)?;
            if text.is_empty() {
                return Err(Error::EmptyVersionFile(candidate));
            }
            return Ok(Some((candidate, text)));
        }
        if !dir.pop() {
            return Ok(None);
        }
    }
}

fn strip_go_prefix(input: &str) -> &str {
    let Some(rest) = input.get(2..) else {
        return input;
    };
    if input.as_bytes()[..2].eq_ignore_ascii_case(b"go")
        && rest.starts_with(|c: char| c.is_ascii_digit())
    {
        rest
    } else {
        input
    }
}

fn split_number(input: &str) -> Result<(u64, &str), ()> {
    let digits = input.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 {
        return Err(());
    }
    if digits > 1 && input.as_bytes()[0] == b'0' {
        return Err(());
    }
    let (number, rest) = input.split_at(digits);
    let value = number.parse().map_err(|_| ())?;
    Ok((value, rest))
}

fn parse_rest(major: u64, minor: u64, rest: &str) -> Result<Version, ()> {
    if let Some(after_dot) = rest.strip_prefix('.') {
        let (patch, after_patch) = split_number(after_dot)?;
        let pre = parse_pre(after_patch)?;
        return Ok(Version {
            major,
            minor,
            patch: Some(patch),
            pre,
        });
    }
    let pre = parse_pre(rest)?;
    if pre.is_none() {
        return Err(());
    }
    Ok(Version {
        major,
        minor,
        patch: None,
        pre,
    })
}

fn parse_pre(input: &str) -> Result<Option<Pre>, ()> {
    if input.is_empty() {
        return Ok(None);
    }
    let (kind, rest) = if let Some(rest) = input.strip_prefix("rc") {
        (PreKind::Rc, rest)
    } else if let Some(rest) = input.strip_prefix("beta") {
        (PreKind::Beta, rest)
    } else {
        return Err(());
    };
    let (n, leftover) = split_number(rest)?;
    if !leftover.is_empty() {
        return Err(());
    }
    Ok(Some(Pre { kind, n }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{touch_sdk, TempDir};
    use std::fs;

    fn exact(input: &str) -> Version {
        match parse_user_spec(input).unwrap() {
            VersionQuery::Exact(version) => version,
            VersionQuery::Minor { .. } | VersionQuery::Latest => panic!("expected exact version"),
        }
    }

    #[test]
    fn parses_go_prefix_patch_and_prerelease() {
        assert_eq!(exact("go1.23.4").to_string(), "1.23.4");
        assert_eq!(exact("GO1.23.4").to_string(), "1.23.4");
        assert_eq!(exact("1.23.4").to_string(), "1.23.4");
        assert_eq!(exact("go1.23rc2").to_string(), "1.23rc2");
        assert_eq!(exact("1.23beta1").to_string(), "1.23beta1");
        assert_eq!(exact("1.23.4rc1").to_string(), "1.23.4rc1");
        assert!(matches!(
            parse_user_spec("1.23").unwrap(),
            VersionQuery::Minor {
                major: 1,
                minor: 23
            }
        ));
        assert!(parse_user_spec("1").is_err());
        assert!(parse_user_spec("latest").is_err());
        assert!(parse_user_spec("LATEST").is_err());
        assert!(matches!(
            parse_install_spec(" latest ").unwrap(),
            VersionQuery::Latest
        ));
        assert!(matches!(
            parse_install_spec("LATEST").unwrap(),
            VersionQuery::Latest
        ));
        assert!(matches!(
            parse_install_spec("1.23").unwrap(),
            VersionQuery::Minor {
                major: 1,
                minor: 23
            }
        ));
        assert!(parse_user_spec("v1.23.4").is_err());
        assert!(parse_user_spec("1.23.").is_err());
        assert!(parse_user_spec("01.2.3").is_err());
        assert!(parse_user_spec("1.23.4rc").is_err());
        assert!(parse_release_version("1.23").is_err());
    }

    #[test]
    fn orders_patches_numerically_and_releases_above_rc() {
        assert!(exact("1.23.10") > exact("1.23.9"));
        assert!(exact("1.23.0") > exact("1.23rc1"));
        assert!(exact("1.23rc2") > exact("1.23rc1"));
        assert!(exact("1.23rc1") > exact("1.23beta1"));
        assert!(exact("1.24.0") > exact("1.23.9"));
    }

    #[test]
    fn minor_query_picks_highest_installed_patch() {
        let installed = vec![
            exact("1.23.0"),
            exact("1.23.10"),
            exact("1.23.9"),
            exact("1.23rc2"),
            exact("1.22.8"),
        ];
        let query = parse_user_spec("1.23").unwrap();
        assert_eq!(
            select_installed(&query, &installed).unwrap().to_string(),
            "1.23.10"
        );
        assert_eq!(
            select_installed(&parse_user_spec("1.23.9").unwrap(), &installed)
                .unwrap()
                .to_string(),
            "1.23.9"
        );
        assert!(select_installed(&parse_user_spec("1.21.0").unwrap(), &installed).is_err());
        assert!(select_installed(&parse_user_spec("1.21").unwrap(), &installed).is_err());
    }

    #[test]
    fn resolution_order_is_env_then_project_then_global() {
        let root = TempDir::new();
        let project = TempDir::new();
        let nested = project.path().join("a").join("b");
        fs::create_dir_all(&nested).unwrap();
        touch_sdk(root.path(), "1.2.0");
        touch_sdk(root.path(), "1.2.3");
        touch_sdk(root.path(), "1.4.0");
        fs::write(project.path().join(".go-version"), "1.2.3\n").unwrap();
        fs::write(root.path().join("version"), "go1.4.0\n").unwrap();

        let from_env = resolve(&nested, root.path(), Some("1.2.0")).unwrap();
        assert_eq!(from_env.version.to_string(), "1.2.0");
        assert_eq!(from_env.origin, Origin::Env);

        let from_file = resolve(&nested, root.path(), Some("   ")).unwrap();
        assert_eq!(from_file.version.to_string(), "1.2.3");
        assert_eq!(
            from_file.origin,
            Origin::Project(project.path().join(".go-version"))
        );

        let outside = TempDir::new();
        let from_global = resolve(outside.path(), root.path(), None).unwrap();
        assert_eq!(from_global.version.to_string(), "1.4.0");
        assert_eq!(
            from_global.origin,
            Origin::Global(root.path().join("version"))
        );
    }

    #[test]
    fn nearer_pin_wins_and_invalid_pin_does_not_fall_through() {
        let root = TempDir::new();
        let project = TempDir::new();
        let child = project.path().join("child");
        fs::create_dir_all(&child).unwrap();
        touch_sdk(root.path(), "1.2.3");
        touch_sdk(root.path(), "1.8.0");
        fs::write(project.path().join(".go-version"), "1.8.0\n").unwrap();
        fs::write(child.join(".go-version"), "1.2.3\n").unwrap();
        fs::write(root.path().join("version"), "1.8.0\n").unwrap();

        let resolved = resolve(&child, root.path(), None).unwrap();
        assert_eq!(resolved.version.to_string(), "1.2.3");

        fs::write(child.join(".go-version"), "nope\n").unwrap();
        assert!(matches!(
            resolve(&child, root.path(), None),
            Err(Error::BadVersion(_))
        ));
    }

    #[test]
    fn minor_pin_uses_highest_patch_and_missing_pin_errors() {
        let root = TempDir::new();
        let project = TempDir::new();
        touch_sdk(root.path(), "1.23.0");
        touch_sdk(root.path(), "1.23.4");
        touch_sdk(root.path(), "1.23.10");
        fs::write(project.path().join(".go-version"), "go1.23\n").unwrap();
        let resolved = resolve(project.path(), root.path(), None).unwrap();
        assert_eq!(resolved.version.to_string(), "1.23.10");

        let empty = TempDir::new();
        let err = resolve(empty.path(), root.path(), None).unwrap_err();
        assert_eq!(
            err.to_string(),
            "No Go version is set. Run gv use <version> or gv use --global <version>"
        );
    }

    #[test]
    fn project_pin_stops_even_when_that_version_is_not_installed() {
        let root = TempDir::new();
        let project = TempDir::new();
        touch_sdk(root.path(), "1.4.0");
        fs::write(root.path().join("version"), "1.4.0\n").unwrap();
        fs::write(project.path().join(".go-version"), "1.2.3\n").unwrap();
        let err = resolve(project.path(), root.path(), None).unwrap_err();
        assert_eq!(
            err.to_string(),
            "Go 1.2.3 is not installed. Run gv install 1.2.3"
        );
    }

    #[test]
    fn exact_version_uses_its_own_sdk_and_a_supplied_list() {
        let root = TempDir::new();
        let project = TempDir::new();
        touch_sdk(root.path(), "1.2.3");
        touch_sdk(root.path(), "9.9.9");
        fs::write(project.path().join(".go-version"), "1.2.3\n").unwrap();

        let resolved = resolve(project.path(), root.path(), None).unwrap();
        assert_eq!(resolved.version.to_string(), "1.2.3");

        let installed = installed_versions(root.path()).unwrap();
        let from_list =
            resolve_using(project.path(), root.path(), None, Some(&installed)).unwrap();
        assert_eq!(from_list.version.to_string(), "1.2.3");

        let err = resolve_using(project.path(), root.path(), None, Some(&[])).unwrap_err();
        assert_eq!(err.to_string(), "Go 1.2.3 is not installed. Run gv install 1.2.3");
    }

    #[test]
    fn empty_project_pin_is_an_error() {
        let root = TempDir::new();
        let project = TempDir::new();
        fs::write(project.path().join(".go-version"), " \n").unwrap();
        assert!(matches!(
            resolve(project.path(), root.path(), None),
            Err(Error::EmptyVersionFile(_))
        ));
    }
}
