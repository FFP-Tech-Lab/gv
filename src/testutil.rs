use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static COUNTER: AtomicU64 = AtomicU64::new(0);

pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new() -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("gv-test-{nanos}-{n}"));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub fn touch_sdk(root: &Path, version: &str) {
    let bin = root.join("versions").join(version).join("go").join("bin");
    fs::create_dir_all(&bin).unwrap();
    fs::write(bin.join("go"), b"#!/bin/sh\n").unwrap();
    fs::write(bin.join("gofmt"), b"#!/bin/sh\n").unwrap();
}
