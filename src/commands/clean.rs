use std::fs;
use std::path::Path;

use crate::error::Error;
use crate::store;

pub fn run(root: &Path) -> Result<String, Error> {
    let dir = store::archive_cache_dir(root);
    if !dir.is_dir() {
        return Ok("No cached archives\n".to_string());
    }
    let mut removed = 0u64;
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() && !file_type.is_symlink() {
            fs::remove_dir_all(&path)?;
        } else {
            fs::remove_file(&path)?;
        }
        removed += 1;
    }
    if removed == 0 {
        Ok("No cached archives\n".to_string())
    } else if removed == 1 {
        Ok("Removed 1 cached archive\n".to_string())
    } else {
        Ok(format!("Removed {removed} cached archives\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{touch_sdk, TempDir};
    use std::fs;

    #[test]
    fn removes_archive_cache_only() {
        let root = TempDir::new();
        touch_sdk(root.path(), "1.23.4");
        let cache = store::cache_dir(root.path());
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("index.json"), "{}\n").unwrap();
        fs::write(cache.join("index.url"), "https://example.test\n").unwrap();
        let archives = store::archive_cache_dir(root.path());
        fs::create_dir_all(&archives).unwrap();
        fs::write(archives.join("go1.23.4.linux-amd64.tar.gz"), b"tarball").unwrap();
        fs::write(archives.join(".partial-go1.23.4.tar.gz-1-1"), b"partial").unwrap();

        let text = run(root.path()).unwrap();
        assert_eq!(text, "Removed 2 cached archives\n");
        assert!(!archives.join("go1.23.4.linux-amd64.tar.gz").exists());
        assert!(!archives.join(".partial-go1.23.4.tar.gz-1-1").exists());
        assert_eq!(fs::read(cache.join("index.json")).unwrap(), b"{}\n");
        assert!(store::tool_path(root.path(), "1.23.4", "go").is_file());

        assert_eq!(run(root.path()).unwrap(), "No cached archives\n");
    }

    #[test]
    fn missing_cache_is_success() {
        let root = TempDir::new();
        touch_sdk(root.path(), "1.2.3");
        assert_eq!(run(root.path()).unwrap(), "No cached archives\n");
        assert!(store::tool_path(root.path(), "1.2.3", "go").is_file());
    }
}
