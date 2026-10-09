pub mod clean;
pub mod completions;
pub mod current;
pub mod init;
pub mod install;
pub mod list;
pub mod shell;
pub mod uninstall;
pub mod which;

#[path = "use.rs"]
pub mod use_version;

use crate::error::Error;

pub fn take_version(version: Option<String>, unset: bool) -> Result<Option<String>, Error> {
    match (version, unset) {
        (Some(_), true) => Err(Error::VersionAndUnset),
        (None, false) => Err(Error::VersionRequired),
        (Some(version), false) => Ok(Some(version)),
        (None, true) => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::take_version;

    #[test]
    fn version_and_unset_are_exclusive() {
        assert!(take_version(Some("1.2.3".into()), true).is_err());
        assert!(take_version(None, false).is_err());
        assert_eq!(take_version(None, true).unwrap(), None);
        assert_eq!(
            take_version(Some("1.2.3".into()), false)
                .unwrap()
                .as_deref(),
            Some("1.2.3")
        );
    }
}
