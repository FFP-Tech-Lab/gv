use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("gv-cli-{nanos}-{n}"));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn gv(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_gv"));
    command.env("GV_ROOT", root);
    command.env_remove("GV_VERSION");
    command.env_remove("GOROOT");
    command.env_remove("GOTOOLCHAIN");
    command.env("GV_INDEX_URL", "http://127.0.0.1:9/index.json");
    command.env("GV_MIRROR", "http://127.0.0.1:9/dl");
    command
}

fn write_exe(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).unwrap();
}

fn install_fake_sdk(root: &Path, version: &str) {
    let script = "#!/bin/sh\n\
printf 'tool=%s\\n' \"$(basename \"$0\")\"\n\
if [ -n \"${GOROOT+x}\" ]; then printf 'GOROOT=%s\\n' \"$GOROOT\"; else printf 'GOROOT=unset\\n'; fi\n\
printf 'GOTOOLCHAIN=%s\\n' \"${GOTOOLCHAIN-unset}\"\n\
printf 'ARGS=%s\\n' \"$*\"\n";
    let bin = root.join("versions").join(version).join("go").join("bin");
    write_exe(&bin.join("go"), script);
    write_exe(&bin.join("gofmt"), script);
}

#[test]
fn init_prints_snippet_and_rejects_other_shells() {
    let root = TempDir::new();
    let bash = gv(root.path()).arg("init").arg("bash").output().unwrap();
    assert!(
        bash.status.success(),
        "{}",
        String::from_utf8_lossy(&bash.stderr)
    );
    let text = String::from_utf8(bash.stdout.clone()).unwrap();
    assert_eq!(
        text,
        "export GV_ROOT=\"${GV_ROOT:-$HOME/.gv}\"\nexport PATH=\"$GV_ROOT/shims:$GV_ROOT/bin:$PATH\"\n"
    );
    let zsh = gv(root.path()).arg("init").arg("zsh").output().unwrap();
    assert_eq!(zsh.stdout, bash.stdout);
    assert!(root.path().join("shims/go").exists());
    assert!(root.path().join("shims/gofmt").exists());

    let fish = gv(root.path()).arg("init").arg("fish").output().unwrap();
    assert!(!fish.status.success());
    assert!(String::from_utf8_lossy(&fish.stderr).contains("bash"));
    let powershell = gv(root.path())
        .arg("init")
        .arg("powershell")
        .output()
        .unwrap();
    assert!(!powershell.status.success());
}

#[test]
fn use_current_list_and_resolution_order() {
    let root = TempDir::new();
    let project = TempDir::new();
    let nested = project.path().join("a").join("b");
    fs::create_dir_all(&nested).unwrap();
    install_fake_sdk(root.path(), "1.23.0");
    install_fake_sdk(root.path(), "1.23.10");
    install_fake_sdk(root.path(), "1.22.2");
    fs::write(
        project.path().join("go.mod"),
        "module example\n\ntoolchain go1.99.0\n",
    )
    .unwrap();

    let use_local = gv(root.path())
        .current_dir(project.path())
        .args(["use", "go1.23"])
        .output()
        .unwrap();
    assert!(
        use_local.status.success(),
        "{}",
        String::from_utf8_lossy(&use_local.stderr)
    );
    assert_eq!(
        fs::read_to_string(project.path().join(".go-version")).unwrap(),
        "1.23\n"
    );

    let current = gv(root.path())
        .current_dir(&nested)
        .arg("current")
        .output()
        .unwrap();
    assert!(
        current.status.success(),
        "{}",
        String::from_utf8_lossy(&current.stderr)
    );
    let stdout = String::from_utf8(current.stdout).unwrap();
    assert!(stdout.starts_with("1.23.10（.go-version:"), "{stdout}");

    let overridden = gv(root.path())
        .current_dir(&nested)
        .env("GV_VERSION", "1.22.2")
        .arg("current")
        .output()
        .unwrap();
    let stdout = String::from_utf8(overridden.stdout).unwrap();
    assert!(
        stdout.starts_with("1.22.2（环境变量 GV_VERSION）"),
        "{stdout}"
    );

    let list = gv(root.path())
        .current_dir(&nested)
        .arg("list")
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8(list.stdout).unwrap(),
        "* 1.23.10\n  1.23.0\n  1.22.2\n"
    );

    let outside = TempDir::new();
    let global = gv(root.path())
        .current_dir(outside.path())
        .args(["use", "--global", "1.22.2"])
        .output()
        .unwrap();
    assert!(
        global.status.success(),
        "{}",
        String::from_utf8_lossy(&global.stderr)
    );
    let current = gv(root.path())
        .current_dir(outside.path())
        .arg("current")
        .output()
        .unwrap();
    let stdout = String::from_utf8(current.stdout).unwrap();
    assert!(stdout.starts_with("1.22.2（全局:"), "{stdout}");
}

#[test]
fn missing_version_has_no_system_go_fallback() {
    let root = TempDir::new();
    let cwd = TempDir::new();
    let current = gv(root.path())
        .current_dir(cwd.path())
        .arg("current")
        .output()
        .unwrap();
    assert!(!current.status.success());
    let stderr = String::from_utf8_lossy(&current.stderr);
    assert!(stderr.contains("gv use <version>"));
    assert!(stderr.contains("gv use --global"));
}

#[test]
fn uninstall_blocks_on_global_pin_until_force() {
    let root = TempDir::new();
    let cwd = TempDir::new();
    install_fake_sdk(root.path(), "1.21.5");
    let use_global = gv(root.path())
        .current_dir(cwd.path())
        .args(["use", "--global", "1.21.5"])
        .output()
        .unwrap();
    assert!(use_global.status.success());

    let blocked = gv(root.path())
        .args(["uninstall", "1.21.5"])
        .output()
        .unwrap();
    assert!(!blocked.status.success());
    assert!(String::from_utf8_lossy(&blocked.stderr).contains("--force"));
    assert!(root.path().join("versions/1.21.5/go/bin/go").is_file());

    let forced = gv(root.path())
        .args(["uninstall", "1.21.5", "--force"])
        .output()
        .unwrap();
    assert!(
        forced.status.success(),
        "{}",
        String::from_utf8_lossy(&forced.stderr)
    );
    assert!(!root.path().join("versions/1.21.5").exists());
    assert!(!root.path().join("version").exists());
}

#[test]
fn already_installed_does_not_contact_the_network() {
    let root = TempDir::new();
    install_fake_sdk(root.path(), "1.2.3");
    let output = gv(root.path())
        .args(["install", "go1.2.3"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "Go 1.2.3 已安装\n"
    );
    assert!(output.stderr.is_empty());

    let quiet = gv(root.path())
        .args(["install", "--quiet", "1.2.3"])
        .output()
        .unwrap();
    assert!(
        quiet.status.success(),
        "{}",
        String::from_utf8_lossy(&quiet.stderr)
    );
    assert_eq!(
        String::from_utf8(quiet.stdout).unwrap(),
        "Go 1.2.3 已安装\n"
    );
    assert!(quiet.stderr.is_empty());
}

#[test]
fn install_accepts_several_versions_and_skips_each_without_network() {
    let root = TempDir::new();
    install_fake_sdk(root.path(), "1.22.5");
    install_fake_sdk(root.path(), "1.23.4");

    let output = gv(root.path())
        .args(["install", "1.22.5", "go1.23.4"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "Go 1.22.5 已安装\nGo 1.23.4 已安装\n"
    );
    assert!(output.stderr.is_empty());

    let quiet = gv(root.path())
        .args(["install", "--quiet", "1.23.4", "1.22.5"])
        .output()
        .unwrap();
    assert!(
        quiet.status.success(),
        "{}",
        String::from_utf8_lossy(&quiet.stderr)
    );
    assert_eq!(
        String::from_utf8(quiet.stdout).unwrap(),
        "Go 1.23.4 已安装\nGo 1.22.5 已安装\n"
    );

    let missing = gv(root.path()).arg("install").output().unwrap();
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("VERSION"));
}

#[test]
fn install_reports_each_failure_and_still_skips_installed_versions() {
    let root = TempDir::new();
    install_fake_sdk(root.path(), "1.22.5");
    install_fake_sdk(root.path(), "1.23.4");

    let output = gv(root.path())
        .args(["install", "1.23.4", "nope", "1.22.5", "also-bad"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "Go 1.23.4 已安装\nGo 1.22.5 已安装\n"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    let lines: Vec<&str> = stderr.lines().collect();
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("gv: ") && line.contains("nope")),
        "{stderr}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("gv: ") && line.contains("also-bad")),
        "{stderr}"
    );
    assert!(stderr.contains("无法识别的版本号"), "{stderr}");
    assert!(root.path().join("versions/1.22.5/go/bin/go").is_file());
    assert!(root.path().join("versions/1.23.4/go/bin/go").is_file());
}

#[test]
fn shim_execs_sdk_and_controls_goroot_and_gotoolchain() {
    let root = TempDir::new();
    let cwd = TempDir::new();
    install_fake_sdk(root.path(), "1.2.3");
    let primed = gv(root.path())
        .current_dir(cwd.path())
        .env("GV_VERSION", "1.2.3")
        .arg("current")
        .output()
        .unwrap();
    assert!(
        primed.status.success(),
        "{}",
        String::from_utf8_lossy(&primed.stderr)
    );

    let shim = root.path().join("shims/go");
    let output = Command::new(&shim)
        .current_dir(cwd.path())
        .env("GV_ROOT", root.path())
        .env("GV_VERSION", "1.2.3")
        .env_remove("GOROOT")
        .env_remove("GOTOOLCHAIN")
        .args(["version", "hello"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("tool=go\n"), "{stdout}");
    assert!(stdout.contains("GOROOT=unset\n"), "{stdout}");
    assert!(stdout.contains("GOTOOLCHAIN=local\n"), "{stdout}");
    assert!(stdout.contains("ARGS=version hello\n"), "{stdout}");

    let kept = Command::new(&shim)
        .current_dir(cwd.path())
        .env("GV_ROOT", root.path())
        .env("GV_VERSION", "1.2.3")
        .env("GOROOT", "/should/not/leak")
        .env("GOTOOLCHAIN", "auto")
        .arg("env")
        .output()
        .unwrap();
    assert!(
        kept.status.success(),
        "{}",
        String::from_utf8_lossy(&kept.stderr)
    );
    let stdout = String::from_utf8(kept.stdout).unwrap();
    assert!(stdout.contains("GOROOT=unset\n"), "{stdout}");
    assert!(stdout.contains("GOTOOLCHAIN=auto\n"), "{stdout}");

    let gofmt = Command::new(root.path().join("shims/gofmt"))
        .current_dir(cwd.path())
        .env("GV_ROOT", root.path())
        .env("GV_VERSION", "1.2.3")
        .env_remove("GOTOOLCHAIN")
        .output()
        .unwrap();
    assert!(
        gofmt.status.success(),
        "{}",
        String::from_utf8_lossy(&gofmt.stderr)
    );
    assert!(String::from_utf8(gofmt.stdout)
        .unwrap()
        .contains("tool=gofmt\n"));

    let missing = Command::new(&shim)
        .current_dir(cwd.path())
        .env("GV_ROOT", root.path())
        .env("GV_VERSION", "9.9.9")
        .output()
        .unwrap();
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("gv install 9.9.9"));
}
