use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

const DEFAULT_CONFIG: &str = "-- fvim configuration\n-- Lua execution and Neovim settings arrive in a later stage.\n";

pub fn config_dir() -> io::Result<PathBuf> {
    if let Some(path) = env::var_os("FVIM_CONFIG_DIR") {
        return Ok(PathBuf::from(path));
    }
    #[cfg(windows)]
    let base = env::var_os("LOCALAPPDATA").map(PathBuf::from);
    #[cfg(not(windows))]
    let base = env::var_os("XDG_CONFIG_HOME")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|p| PathBuf::from(p).join(".config")));
    base.map(|p| p.join("fvim"))
        .ok_or_else(|| io::Error::other("Cannot determine configuration directory."))
}

pub fn init_config() -> io::Result<()> {
    let dir = config_dir()?;
    fs::create_dir_all(&dir)?;
    match OpenOptions::new().write(true).create_new(true).open(dir.join("init.lua")) {
        Ok(mut file) => file.write_all(DEFAULT_CONFIG.as_bytes())?,
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    if env::var_os("FVIM_NO_PATH").is_none() {
        if let Some(bin) = env::var_os("FVIM_INSTALL_BIN") {
            persist_path(Path::new(&bin))?;
        }
    }
    Ok(())
}

pub fn install() -> io::Result<()> {
    let explicit = env::var_os("FVIM_PREFIX").map(PathBuf::from);
    #[cfg(windows)]
    let prefix = explicit.unwrap_or(config_dir()?);
    #[cfg(not(windows))]
    let prefix = if let Some(prefix) = explicit {
        prefix
    } else if env::var_os("SUDO_USER").is_some() || env::var_os("USER").as_deref() == Some(std::ffi::OsStr::new("root")) {
        PathBuf::from("/usr/local")
    } else {
        PathBuf::from(env::var_os("HOME").ok_or_else(|| io::Error::other("HOME is missing."))?).join(".local")
    };
    let bin = prefix.join("bin");
    fs::create_dir_all(&bin)?;
    let source = env::current_exe()?;
    let target = bin.join(if cfg!(windows) { "fvim.exe" } else { "fvim" });
    let temporary = bin.join(format!(".fvim-install-{}", std::process::id()));
    fs::copy(&source, &temporary)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o755))?;
    }
    if let Err(error) = fs::rename(&temporary, &target) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    let bin = fs::canonicalize(bin)?;
    #[cfg(windows)]
    let bin = {
        let text = bin.to_string_lossy();
        if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
            PathBuf::from(format!(r"\\{unc}"))
        } else if let Some(local) = text.strip_prefix(r"\\?\") {
            PathBuf::from(local)
        } else {
            bin
        }
    };
    #[cfg(unix)]
    if let Some(user) = env::var_os("SUDO_USER") {
        let status = Command::new("sudo")
            .args(["-H", "-u"])
            .arg(user)
            .arg("--")
            .arg(&target)
            .arg("--init-config")
            .env("FVIM_INSTALL_BIN", &bin)
            .status()?;
        if !status.success() {
            return Err(io::Error::other("User configuration setup failed."));
        }
        println!("Installed {}", target.display());
        return Ok(());
    }
    init_config()?;
    if env::var_os("FVIM_NO_PATH").is_none() {
        persist_path(&bin)?;
    }
    println!("Installed {}", target.display());
    println!("Config {}", config_dir()?.join("init.lua").display());
    println!("Open a new terminal if PATH changed.");
    Ok(())
}

#[cfg(windows)]
fn persist_path(bin: &Path) -> io::Result<()> {
    let status = Command::new("powershell.exe")
        .args(["-NoProfile", "-Command", r#"
$ErrorActionPreference = 'Stop'
$bin = $env:FVIM_INSTALL_BIN
$path = [Environment]::GetEnvironmentVariable('Path', 'User')
$parts = @($path -split ';' | Where-Object { $_ })
if (-not ($parts | Where-Object { $_.TrimEnd('\') -ieq $bin.TrimEnd('\') })) {
    [Environment]::SetEnvironmentVariable('Path', (($parts + $bin) -join ';'), 'User')
}
"#])
        .env("FVIM_INSTALL_BIN", bin)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other("Failed to persist user PATH."))
    }
}

#[cfg(not(windows))]
fn persist_path(bin: &Path) -> io::Result<()> {
    if env::split_paths(&env::var_os("PATH").unwrap_or_default()).any(|p| p == bin) {
        return Ok(());
    }
    let home = PathBuf::from(env::var_os("HOME").ok_or_else(|| io::Error::other("HOME is missing."))?);
    let quoted = bin.to_string_lossy().replace('\'', "'\\''");
    let entry = format!("\n# fvim PATH\nexport PATH='{quoted}':\"$PATH\"\n");
    for name in [".profile", ".bashrc", ".zshrc"] {
        let path = home.join(name);
        let existing = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e),
        };
        if !existing.contains(&entry) {
            OpenOptions::new().create(true).append(true).open(path)?.write_all(entry.as_bytes())?;
        }
    }
    let fish_root = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    let fish = fish_root.join("fish/conf.d");
    fs::create_dir_all(&fish)?;
    // fish single-quoted strings escape backslashes and quotes directly.
    let quoted = bin.to_string_lossy().replace('\\', "\\\\").replace('\'', "\\'");
    fs::write(fish.join("fvim-path.fish"), format!("fish_add_path '{quoted}'\n"))?;
    Ok(())
}
