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
    assert!(stdout.starts_with("1.23.10 (.go-version:"), "{stdout}");

    let overridden = gv(root.path())
        .current_dir(&nested)
        .env("GV_VERSION", "1.22.2")
        .arg("current")
        .output()
        .unwrap();
    let stdout = String::from_utf8(overridden.stdout).unwrap();
    assert!(
        stdout.starts_with("1.22.2 (environment variable GV_VERSION)"),
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
    assert!(stdout.starts_with("1.22.2 (global:"), "{stdout}");
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
fn uninstall_removes_each_given_version() {
    let root = TempDir::new();
    install_fake_sdk(root.path(), "1.22.5");
    install_fake_sdk(root.path(), "1.23.4");

    let output = gv(root.path())
        .args(["uninstall", "1.22.5", "go1.23.4"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "Uninstalled Go 1.22.5\nUninstalled Go 1.23.4\n"
    );
    assert!(output.stderr.is_empty());
    assert!(!root.path().join("versions/1.22.5").exists());
    assert!(!root.path().join("versions/1.23.4").exists());
}

#[test]
fn uninstall_continues_after_failure_and_notices_missing_versions() {
    let root = TempDir::new();
    let cwd = TempDir::new();
    install_fake_sdk(root.path(), "1.22.5");
    install_fake_sdk(root.path(), "1.23.4");
    install_fake_sdk(root.path(), "1.21.0");
    let use_global = gv(root.path())
        .current_dir(cwd.path())
        .args(["use", "--global", "1.23.4"])
        .output()
        .unwrap();
    assert!(
        use_global.status.success(),
        "{}",
        String::from_utf8_lossy(&use_global.stderr)
    );

    let output = gv(root.path())
        .args(["uninstall", "1.22.5", "1.23", "9.9.9", "1.23.4", "1.21.0"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "Uninstalled Go 1.22.5\nGo 9.9.9 is not installed\nUninstalled Go 1.21.0\n"
    );
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        "gv: Failed to uninstall Go 1.23: Specify a full version, not just 1.23\n\
         gv: Failed to uninstall Go 1.23.4: The global version points at 1.23.4. To uninstall it, run gv uninstall 1.23.4 --force\n"
    );
    assert!(!root.path().join("versions/1.22.5").exists());
    assert!(root.path().join("versions/1.23.4/go/bin/go").is_file());
    assert!(!root.path().join("versions/1.21.0").exists());
    assert!(root.path().join("version").is_file());
}

#[test]
fn uninstall_force_clears_global_pin_among_several_versions() {
    let root = TempDir::new();
    let cwd = TempDir::new();
    install_fake_sdk(root.path(), "1.22.5");
    install_fake_sdk(root.path(), "1.23.4");
    let use_global = gv(root.path())
        .current_dir(cwd.path())
        .args(["use", "--global", "1.23.4"])
        .output()
        .unwrap();
    assert!(
        use_global.status.success(),
        "{}",
        String::from_utf8_lossy(&use_global.stderr)
    );

    let output = gv(root.path())
        .args(["uninstall", "--force", "1.22.5", "1.23.4"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "Uninstalled Go 1.22.5\nUninstalled Go 1.23.4 and cleared the global version\n"
    );
    assert!(output.stderr.is_empty());
    assert!(!root.path().join("versions/1.22.5").exists());
    assert!(!root.path().join("versions/1.23.4").exists());
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
        "Go 1.2.3 is already installed\n"
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
        "Go 1.2.3 is already installed\n"
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
        "Go 1.22.5 is already installed\nGo 1.23.4 is already installed\n"
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
        "Go 1.23.4 is already installed\nGo 1.22.5 is already installed\n"
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
        "Go 1.23.4 is already installed\nGo 1.22.5 is already installed\n"
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
    assert!(stderr.contains("Unrecognized version"), "{stderr}");
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

fn go_os_arch() -> (&'static str, &'static str) {
    let os = match std::env::consts::OS {
        "linux" => "linux",
        "macos" | "darwin" => "darwin",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" | "amd64" => "amd64",
        "aarch64" | "arm64" => "arm64",
        other => other,
    };
    (os, arch)
}

fn seed_index(root: &Path, body: &str) {
    seed_index_at(root, body, SystemTime::now());
}

fn seed_index_at(root: &Path, body: &str, fetched: SystemTime) {
    let cache = root.join("cache");
    fs::create_dir_all(&cache).unwrap();
    fs::write(cache.join("index.json"), body).unwrap();
    fs::write(cache.join("index.url"), "http://127.0.0.1:9/index.json\n").unwrap();
    let secs = fetched.duration_since(UNIX_EPOCH).unwrap().as_secs();
    fs::write(cache.join("index.fetched"), format!("{secs}\n")).unwrap();
}

fn sample_remote_index() -> String {
    let (os, arch) = go_os_arch();
    format!(
        r#"[
          {{"version":"go1.22.5","stable":true,"files":[{{"filename":"go1.22.5.{os}-{arch}.tar.gz","os":"{os}","arch":"{arch}","sha256":"abc","kind":"archive"}}]}},
          {{"version":"go1.24.0","stable":true,"files":[{{"filename":"go1.24.0.{os}-{arch}.tar.gz","os":"{os}","arch":"{arch}","sha256":"def","kind":"archive"}}]}},
          {{"version":"go1.25rc1","stable":false,"files":[{{"filename":"go1.25rc1.{os}-{arch}.tar.gz","os":"{os}","arch":"{arch}","sha256":"ghi","kind":"archive"}}]}}
        ]"#
    )
}

#[test]
fn which_prints_source_and_tool_paths() {
    let root = TempDir::new();
    let project = TempDir::new();
    install_fake_sdk(root.path(), "1.23.0");
    install_fake_sdk(root.path(), "1.23.10");
    let used = gv(root.path())
        .current_dir(project.path())
        .args(["use", "1.23"])
        .output()
        .unwrap();
    assert!(used.status.success());

    let output = gv(root.path())
        .current_dir(project.path())
        .arg("which")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("1.23.10 (.go-version:"), "{stdout}");
    assert!(
        stdout.contains(&format!("versions/1.23.10/go/bin/go")),
        "{stdout}"
    );
    assert!(stdout.contains("versions/1.23.10/go/bin/gofmt"), "{stdout}");

    let missing = gv(root.path())
        .current_dir(project.path())
        .env("GV_VERSION", "9.9.9")
        .arg("which")
        .output()
        .unwrap();
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("gv install 9.9.9"));
}

#[test]
fn completions_are_print_only() {
    let root = TempDir::new();
    let bash = gv(root.path())
        .args(["completions", "bash"])
        .output()
        .unwrap();
    assert!(bash.status.success());
    let text = String::from_utf8(bash.stdout).unwrap();
    assert!(text.contains("install"));
    assert!(text.contains("list-remote"));
    assert!(text.contains("gv list"));
    assert!(text.contains("latest"));
    assert!(text.contains("will not modify ~/.bashrc"));
    assert!(!text.contains("cd "));
    assert!(!text.contains(">>"));

    let zsh = gv(root.path())
        .args(["completions", "zsh"])
        .output()
        .unwrap();
    assert!(zsh.status.success());
    let text = String::from_utf8(zsh.stdout).unwrap();
    assert!(text.contains("will not modify ~/.zshrc"));
    assert!(text.contains("gv list"));
    assert!(!text.contains("cd "));

    let fish = gv(root.path())
        .args(["completions", "fish"])
        .output()
        .unwrap();
    assert!(!fish.status.success());
    assert!(String::from_utf8_lossy(&fish.stderr).contains("bash"));
}

#[test]
fn shell_prints_export_or_unset() {
    let root = TempDir::new();
    let cwd = TempDir::new();
    let export = gv(root.path())
        .current_dir(cwd.path())
        .args(["shell", "go1.23.4"])
        .output()
        .unwrap();
    assert!(export.status.success());
    let export_stdout = String::from_utf8(export.stdout).unwrap();
    assert_eq!(export_stdout, "export GV_VERSION=1.23.4\n");
    assert!(!export_stdout.contains("cd "));

    let unset = gv(root.path())
        .current_dir(cwd.path())
        .args(["shell", "--unset"])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8(unset.stdout).unwrap(),
        "unset GV_VERSION\n"
    );

    let both = gv(root.path())
        .args(["shell", "--unset", "1.2.3"])
        .output()
        .unwrap();
    assert!(!both.status.success());
    assert!(String::from_utf8_lossy(&both.stderr)
        .contains("Cannot specify a version and --unset together"));

    let latest = gv(root.path()).args(["shell", "latest"]).output().unwrap();
    assert!(!latest.status.success());
    assert!(String::from_utf8_lossy(&latest.stderr).contains("Unrecognized version"));
}

#[test]
fn use_unset_deletes_only_the_local_pin() {
    let root = TempDir::new();
    let parent = TempDir::new();
    let child = parent.path().join("child");
    fs::create_dir_all(&child).unwrap();
    install_fake_sdk(root.path(), "1.23.4");
    install_fake_sdk(root.path(), "1.22.5");
    fs::write(parent.path().join(".go-version"), "1.23.4\n").unwrap();
    let used = gv(root.path())
        .current_dir(&child)
        .args(["use", "1.22.5"])
        .output()
        .unwrap();
    assert!(used.status.success());

    let removed = gv(root.path())
        .current_dir(&child)
        .args(["use", "--unset"])
        .output()
        .unwrap();
    assert!(
        removed.status.success(),
        "{}",
        String::from_utf8_lossy(&removed.stderr)
    );
    assert_eq!(
        String::from_utf8(removed.stdout).unwrap(),
        "Removed .go-version from the current directory\n"
    );
    assert!(!child.join(".go-version").exists());
    assert_eq!(
        fs::read_to_string(parent.path().join(".go-version")).unwrap(),
        "1.23.4\n"
    );

    let missing = gv(root.path())
        .current_dir(&child)
        .args(["use", "--unset"])
        .output()
        .unwrap();
    assert!(missing.status.success());
    assert_eq!(
        String::from_utf8(missing.stdout).unwrap(),
        "No .go-version in the current directory\n"
    );

    let global = gv(root.path())
        .current_dir(&child)
        .args(["use", "--global", "1.23.4"])
        .output()
        .unwrap();
    assert!(global.status.success());
    let cleared = gv(root.path())
        .current_dir(&child)
        .args(["use", "--global", "--unset"])
        .output()
        .unwrap();
    assert!(cleared.status.success());
    assert!(!root.path().join("version").exists());
    assert!(parent.path().join(".go-version").is_file());

    let rejected = gv(root.path())
        .current_dir(&child)
        .args(["use", "--unset", "1.2.3"])
        .output()
        .unwrap();
    assert!(!rejected.status.success());
    assert!(parent.path().join(".go-version").is_file());
    assert!(!child.join(".go-version").exists());
}

#[test]
fn list_json_keeps_text_output() {
    let root = TempDir::new();
    let cwd = TempDir::new();
    install_fake_sdk(root.path(), "1.22.1");
    install_fake_sdk(root.path(), "1.23.4");
    fs::write(cwd.path().join(".go-version"), "1.23.4\n").unwrap();

    let text = gv(root.path())
        .current_dir(cwd.path())
        .arg("list")
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8(text.stdout).unwrap(),
        "* 1.23.4\n  1.22.1\n"
    );

    let json = gv(root.path())
        .current_dir(cwd.path())
        .args(["list", "--output", "json"])
        .output()
        .unwrap();
    assert!(json.status.success());
    let stdout = String::from_utf8(json.stdout).unwrap();
    assert!(
        stdout.contains("\"version\":\"1.23.4\"") || stdout.contains("\"version\": \"1.23.4\"")
    );
    assert!(stdout.contains("\"current\":true") || stdout.contains("\"current\": true"));
    assert!(
        stdout.contains("\"current\":\"1.23.4\"") || stdout.contains("\"current\": \"1.23.4\"")
    );
    assert!(!stdout.contains('*'));
    assert!(!stdout.contains('\u{1b}'));
}

#[test]
fn list_output_stays_plain_when_stdout_is_not_a_terminal() {
    let root = TempDir::new();
    let cwd = TempDir::new();
    install_fake_sdk(root.path(), "1.22.1");
    install_fake_sdk(root.path(), "1.23.4");
    fs::write(cwd.path().join(".go-version"), "1.23.4\n").unwrap();
    seed_index(root.path(), &sample_remote_index());

    for term in ["xterm-256color", "dumb"] {
        let list = gv(root.path())
            .current_dir(cwd.path())
            .env("TERM", term)
            .arg("list")
            .output()
            .unwrap();
        assert!(
            list.status.success(),
            "{}",
            String::from_utf8_lossy(&list.stderr)
        );
        let stdout = String::from_utf8(list.stdout).unwrap();
        assert_eq!(stdout, "* 1.23.4\n  1.22.1\n");
        assert!(!stdout.contains('\u{1b}'), "{term}: {stdout}");
        assert!(list.stderr.is_empty(), "{term}");

        let remote = gv(root.path())
            .env("TERM", term)
            .args(["list-remote", "--offline"])
            .output()
            .unwrap();
        assert!(
            remote.status.success(),
            "{}",
            String::from_utf8_lossy(&remote.stderr)
        );
        let stdout = String::from_utf8(remote.stdout).unwrap();
        assert_eq!(stdout, "  1.24.0\n  1.22.5\n");
        assert!(!stdout.contains('\u{1b}'), "{term}: {stdout}");

        let json = gv(root.path())
            .current_dir(cwd.path())
            .env("TERM", term)
            .args(["list", "--output", "json"])
            .output()
            .unwrap();
        let stdout = String::from_utf8(json.stdout).unwrap();
        assert!(stdout.contains("\"current\""));
        assert!(!stdout.contains('\u{1b}'));
        assert!(!stdout.contains("* 1.23.4"));
    }
}

#[test]
fn list_remote_json_reports_stable_from_cache() {
    let root = TempDir::new();
    let cwd = TempDir::new();
    seed_index(root.path(), &sample_remote_index());

    let text = gv(root.path())
        .current_dir(cwd.path())
        .arg("list-remote")
        .output()
        .unwrap();
    assert!(
        text.status.success(),
        "{}",
        String::from_utf8_lossy(&text.stderr)
    );
    assert_eq!(
        String::from_utf8(text.stdout).unwrap(),
        "  1.24.0\n  1.22.5\n"
    );

    let json = gv(root.path())
        .current_dir(cwd.path())
        .args(["list-remote", "--all", "--output", "json"])
        .output()
        .unwrap();
    assert!(
        json.status.success(),
        "{}",
        String::from_utf8_lossy(&json.stderr)
    );
    let stdout = String::from_utf8(json.stdout).unwrap();
    assert!(
        stdout.contains("\"version\":\"1.24.0\"") || stdout.contains("\"version\": \"1.24.0\"")
    );
    assert!(stdout.contains("\"stable\":true") || stdout.contains("\"stable\": true"));
    assert!(stdout.contains("1.25rc1"));
    assert!(stdout.contains("\"stable\":false") || stdout.contains("\"stable\": false"));
    assert!(stdout.contains("\"installed\":false") || stdout.contains("\"installed\": false"));
}

#[test]
fn list_remote_filters_and_marks_installed_versions() {
    let root = TempDir::new();
    seed_index(root.path(), &sample_remote_index());
    install_fake_sdk(root.path(), "1.24.0");

    let exact = gv(root.path())
        .args(["list-remote", "1.24"])
        .output()
        .unwrap();
    assert!(
        exact.status.success(),
        "{}",
        String::from_utf8_lossy(&exact.stderr)
    );
    assert_eq!(String::from_utf8(exact.stdout).unwrap(), "* 1.24.0\n");

    let minor = gv(root.path())
        .args(["list-remote", "1.22"])
        .output()
        .unwrap();
    assert!(
        minor.status.success(),
        "{}",
        String::from_utf8_lossy(&minor.stderr)
    );
    assert_eq!(String::from_utf8(minor.stdout).unwrap(), "  1.22.5\n");
}

#[test]
fn list_remote_offline_uses_a_stale_cache_without_a_warning() {
    let root = TempDir::new();
    seed_index_at(root.path(), &sample_remote_index(), UNIX_EPOCH);
    let started = std::time::Instant::now();
    let text = gv(root.path())
        .args(["list-remote", "--offline"])
        .output()
        .unwrap();
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "offline list-remote contacted the network"
    );
    assert!(
        text.status.success(),
        "{}",
        String::from_utf8_lossy(&text.stderr)
    );
    assert!(text.stderr.is_empty());
    assert_eq!(
        String::from_utf8(text.stdout).unwrap(),
        "  1.24.0\n  1.22.5\n"
    );
}

#[test]
fn install_latest_uses_highest_stable_and_skips_when_present() {
    let root = TempDir::new();
    seed_index(root.path(), &sample_remote_index());
    install_fake_sdk(root.path(), "1.22.5");
    install_fake_sdk(root.path(), "1.24.0");

    let skipped = gv(root.path())
        .args(["install", "latest"])
        .output()
        .unwrap();
    assert!(
        skipped.status.success(),
        "{}",
        String::from_utf8_lossy(&skipped.stderr)
    );
    assert_eq!(
        String::from_utf8(skipped.stdout).unwrap(),
        "Go 1.24.0 is already installed\n"
    );
    assert!(skipped.stderr.is_empty());

    let _ = fs::remove_dir_all(root.path().join("versions/1.24.0"));
    let output = gv(root.path())
        .args(["install", "1.22.5", "latest"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "Go 1.22.5 is already installed\n"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("go1.24.0."), "{stderr}");
    assert!(!stderr.contains("1.25rc1"), "{stderr}");
    assert!(!stderr.contains("Unrecognized version"), "{stderr}");
    assert!(root.path().join("versions/1.22.5/go/bin/go").is_file());
}

#[test]
fn clean_removes_archive_cache_only() {
    let root = TempDir::new();
    install_fake_sdk(root.path(), "1.23.4");
    let cache = root.path().join("cache");
    fs::create_dir_all(cache.join("archives")).unwrap();
    fs::write(cache.join("index.json"), "{}\n").unwrap();
    fs::write(
        cache.join("archives").join("go1.23.4.linux-amd64.tar.gz"),
        b"tarball",
    )
    .unwrap();

    let output = gv(root.path()).arg("clean").output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "Removed 1 cached archive\n"
    );
    assert!(!cache
        .join("archives")
        .join("go1.23.4.linux-amd64.tar.gz")
        .exists());
    assert_eq!(fs::read(cache.join("index.json")).unwrap(), b"{}\n");
    assert!(root.path().join("versions/1.23.4/go/bin/go").is_file());

    let again = gv(root.path()).arg("clean").output().unwrap();
    assert_eq!(
        String::from_utf8(again.stdout).unwrap(),
        "No cached archives\n"
    );
}
