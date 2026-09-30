use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Sandbox(PathBuf);

impl Sandbox {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "fvim-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn installation_is_repeatable_and_preserves_config() {
    let sandbox = Sandbox::new();
    let prefix = sandbox.0.join("prefix with spaces");
    let config = sandbox.0.join("config with spaces");
    for pass in 0..2 {
        let result = Command::new(env!("CARGO_BIN_EXE_fvim"))
            .arg("--install")
            .env("FVIM_PREFIX", &prefix)
            .env("FVIM_CONFIG_DIR", &config)
            .env("FVIM_NO_PATH", "1")
            .env_remove("SUDO_USER")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let init = config.join("init.lua");
        if pass == 0 {
            assert!(init.is_file());
            fs::write(&init, "-- keep my config\n").unwrap();
        } else {
            assert_eq!(fs::read_to_string(init).unwrap(), "-- keep my config\n");
        }
    }
    let executable = prefix
        .join("bin")
        .join(if cfg!(windows) { "fvim.exe" } else { "fvim" });
    let result = Command::new(executable).arg("--version").output().unwrap();
    assert!(result.status.success());
    assert_eq!(
        String::from_utf8(result.stdout).unwrap().trim(),
        "fvim 0.2.0"
    );
}

#[test]
fn config_location_and_noninteractive_failure() {
    let sandbox = Sandbox::new();
    let result = Command::new(env!("CARGO_BIN_EXE_fvim"))
        .arg("--config-path")
        .env("FVIM_CONFIG_DIR", &sandbox.0)
        .output()
        .unwrap();
    assert!(result.status.success());
    assert_eq!(
        String::from_utf8(result.stdout).unwrap().trim(),
        sandbox.0.join("init.lua").to_string_lossy()
    );
    let result = Command::new(env!("CARGO_BIN_EXE_fvim")).output().unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("requires a terminal"));
}

#[test]
fn default_platform_config_location_is_created() {
    let sandbox = Sandbox::new();
    let mut command = Command::new(env!("CARGO_BIN_EXE_fvim"));
    command
        .arg("--init-config")
        .env_remove("FVIM_CONFIG_DIR")
        .env("FVIM_NO_PATH", "1");
    #[cfg(windows)]
    command.env("LOCALAPPDATA", &sandbox.0);
    #[cfg(not(windows))]
    command.env("XDG_CONFIG_HOME", &sandbox.0);
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(sandbox.0.join("fvim/init.lua").is_file());
}
