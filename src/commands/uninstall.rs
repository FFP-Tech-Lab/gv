use std::fs;
use std::path::Path;

use crate::error::Error;
use crate::resolve::{self, Version, VersionQuery};
use crate::store;

pub fn run(root: &Path, requested: &[String], force: bool) -> Result<(), Error> {
    if requested.len() == 1 {
        uninstall_one(root, &requested[0], force)?;
        return Ok(());
    }

    let mut failures = Vec::new();
    for spec in requested {
        match uninstall_one(root, spec, force) {
            Ok(_) => {}
            Err(Error::NotInstalled(name)) => {
                show_uninstall(&format!("Go {name} is not installed"));
            }
            Err(err) => failures.push(format!("Failed to uninstall Go {spec}: {err}")),
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(Error::Failed(failures.join("\n")))
    }
}

fn uninstall_one(root: &Path, requested: &str, force: bool) -> Result<String, Error> {
    let version = resolve::require_exact(requested)?;
    let name = version.to_string();
    if !store::tool_exists(root, &name, "go") {
        return Err(Error::NotInstalled(name));
    }
    let points = global_points_at(root, &version)?;
    if points && !force {
        return Err(Error::UninstallBlocked(name));
    }
    let dir = store::version_dir(root, &name);
    let meta = fs::symlink_metadata(&dir)?;
    if meta.file_type().is_symlink() {
        return Err(Error::UnsafePath(dir.display().to_string()));
    }
    let ui = crate::ui::Ui::detect(false);
    let activity = ui.start(&format!("Uninstalling Go {name}"));
    fs::remove_dir_all(&dir)?;
    if points {
        fs::remove_file(store::global_version_path(root))?;
        let message = format!("Uninstalled Go {name} and cleared the global version");
        activity.finish(crate::ui::Tone::Done, &message);
        return Ok(message);
    }
    let message = format!("Uninstalled Go {name}");
    activity.finish(crate::ui::Tone::Done, &message);
    Ok(message)
}

fn uninstall_tone(message: &str) -> crate::ui::Tone {
    if message.starts_with("Uninstalled Go ") {
        crate::ui::Tone::Done
    } else {
        crate::ui::Tone::Notice
    }
}

fn show_uninstall(message: &str) {
    crate::ui::Ui::detect(false).finish(uninstall_tone(message), message);
}

fn global_points_at(root: &Path, version: &Version) -> Result<bool, Error> {
    let path = store::global_version_path(root);
    if !path.is_file() {
        return Ok(false);
    }
    let text = store::read_version_text(&path)?;
    if text.is_empty() {
        return Ok(false);
    }
    match resolve::parse_user_spec(&text) {
        Ok(VersionQuery::Exact(pinned)) => Ok(pinned == *version),
        _ => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{touch_sdk, TempDir};
    use std::fs;

    #[test]
    fn uninstall_tones_match_the_sentences() {
        assert_eq!(
            uninstall_tone("Uninstalled Go 1.22.0"),
            crate::ui::Tone::Done
        );
        assert_eq!(
            uninstall_tone("Uninstalled Go 1.23.4 and cleared the global version"),
            crate::ui::Tone::Done
        );
        assert_eq!(
            uninstall_tone("Go 9.9.9 is not installed"),
            crate::ui::Tone::Notice
        );
    }

    #[test]
    fn refuses_when_global_points_at_version_unless_forced() {
        let root = TempDir::new();
        touch_sdk(root.path(), "1.23.4");
        touch_sdk(root.path(), "1.22.0");
        fs::write(root.path().join("version"), "go1.23.4\n").unwrap();

        let err = uninstall_one(root.path(), "1.23.4", false).unwrap_err();
        assert!(err.to_string().contains("gv uninstall 1.23.4 --force"));
        assert!(store::tool_exists(root.path(), "1.23.4", "go"));

        let message = uninstall_one(root.path(), "1.22.0", false).unwrap();
        assert_eq!(message, "Uninstalled Go 1.22.0");
        assert!(!store::tool_exists(root.path(), "1.22.0", "go"));
        assert!(store::tool_exists(root.path(), "1.23.4", "go"));

        let forced = uninstall_one(root.path(), "1.23.4", true).unwrap();
        assert_eq!(
            forced,
            "Uninstalled Go 1.23.4 and cleared the global version"
        );
        assert!(!root.path().join("version").exists());
        assert!(!store::version_dir(root.path(), "1.23.4").exists());
    }

    #[test]
    fn minor_version_is_rejected() {
        let root = TempDir::new();
        touch_sdk(root.path(), "1.23.4");
        assert!(matches!(
            uninstall_one(root.path(), "1.23", false),
            Err(Error::NeedExactVersion(_))
        ));
        assert!(matches!(
            run(root.path(), &["1.23".to_string()], false),
            Err(Error::NeedExactVersion(_))
        ));
    }

    #[test]
    fn one_version_still_returns_the_original_error() {
        let root = TempDir::new();
        touch_sdk(root.path(), "1.23.4");
        fs::write(root.path().join("version"), "1.23.4\n").unwrap();

        let err = run(root.path(), &["1.23.4".to_string()], false).unwrap_err();
        assert!(matches!(err, Error::UninstallBlocked(_)));
        assert!(store::tool_exists(root.path(), "1.23.4", "go"));
    }

    #[test]
    fn missing_versions_do_not_stop_the_batch() {
        let root = TempDir::new();
        touch_sdk(root.path(), "1.22.5");

        run(
            root.path(),
            &[
                "9.9.9".to_string(),
                "1.22.5".to_string(),
                "8.8.8".to_string(),
            ],
            false,
        )
        .unwrap();
        assert!(!store::tool_exists(root.path(), "1.22.5", "go"));
    }

    #[test]
    fn continues_after_failure_and_keeps_later_versions_moving() {
        let root = TempDir::new();
        touch_sdk(root.path(), "1.22.5");
        touch_sdk(root.path(), "1.23.4");
        touch_sdk(root.path(), "1.21.0");
        fs::write(root.path().join("version"), "go1.23.4\n").unwrap();

        let err = run(
            root.path(),
            &[
                "1.22.5".to_string(),
                "1.23".to_string(),
                "9.9.9".to_string(),
                "1.23.4".to_string(),
                "1.21.0".to_string(),
            ],
            false,
        )
        .unwrap_err();
        let text = err.to_string();
        assert!(text.contains("Failed to uninstall Go 1.23"));
        assert!(text.contains("not just 1.23"));
        assert!(text.contains("gv uninstall 1.23.4 --force"));
        assert!(!text.contains("9.9.9"));
        assert!(!store::tool_exists(root.path(), "1.22.5", "go"));
        assert!(store::tool_exists(root.path(), "1.23.4", "go"));
        assert!(!store::tool_exists(root.path(), "1.21.0", "go"));
        assert_eq!(
            fs::read_to_string(root.path().join("version")).unwrap(),
            "go1.23.4\n"
        );
    }

    #[test]
    fn force_clears_global_when_it_points_at_one_version() {
        let root = TempDir::new();
        touch_sdk(root.path(), "1.22.5");
        touch_sdk(root.path(), "1.23.4");
        touch_sdk(root.path(), "1.21.0");
        fs::write(root.path().join("version"), "go1.23.4\n").unwrap();

        run(
            root.path(),
            &[
                "1.22.5".to_string(),
                "1.23.4".to_string(),
                "1.21.0".to_string(),
            ],
            true,
        )
        .unwrap();
        assert!(!store::tool_exists(root.path(), "1.22.5", "go"));
        assert!(!store::version_dir(root.path(), "1.23.4").exists());
        assert!(!store::tool_exists(root.path(), "1.21.0", "go"));
        assert!(!root.path().join("version").exists());
    }
}
