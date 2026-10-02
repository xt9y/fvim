use std::{
    io::Write,
    process::{Command, Stdio},
};

fn write_command(program: &str, args: &[&str], text: &str) -> Result<(), String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    child
        .stdin
        .take()
        .ok_or("Clipboard process has no stdin")?
        .write_all(text.as_bytes())
        .map_err(|e| e.to_string())?;
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}

fn read_command(program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    if output.status.success() {
        String::from_utf8(output.stdout).map_err(|e| e.to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}

#[cfg(target_os = "macos")]
pub fn set(text: &str) -> Result<(), String> {
    write_command("pbcopy", &[], text)
}

#[cfg(target_os = "macos")]
pub fn get() -> Result<String, String> {
    read_command("pbpaste", &[])
}

#[cfg(windows)]
pub fn set(text: &str) -> Result<(), String> {
    write_command(
        "powershell.exe",
        &[
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "$text=[Console]::In.ReadToEnd(); Set-Clipboard -Value $text",
        ],
        text,
    )
}

#[cfg(windows)]
pub fn get() -> Result<String, String> {
    read_command(
        "powershell.exe",
        &[
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "[Console]::Out.Write((Get-Clipboard -Raw))",
        ],
    )
}

#[cfg(all(unix, not(target_os = "macos")))]
pub fn set(text: &str) -> Result<(), String> {
    let candidates: &[(&str, &[&str])] = &[
        ("wl-copy", &[]),
        ("xclip", &["-selection", "clipboard", "-in"]),
        ("xsel", &["--clipboard", "--input"]),
    ];
    let mut last = None;
    for (program, args) in candidates {
        match write_command(program, args, text) {
            Ok(()) => return Ok(()),
            Err(error) => last = Some(error),
        }
    }
    Err(last.unwrap_or_else(|| "No system clipboard utility available".into()))
}

#[cfg(all(unix, not(target_os = "macos")))]
pub fn get() -> Result<String, String> {
    let candidates: &[(&str, &[&str])] = &[
        ("wl-paste", &["--no-newline"]),
        ("xclip", &["-selection", "clipboard", "-out"]),
        ("xsel", &["--clipboard", "--output"]),
    ];
    let mut last = None;
    for (program, args) in candidates {
        match read_command(program, args) {
            Ok(text) => return Ok(text),
            Err(error) => last = Some(error),
        }
    }
    Err(last.unwrap_or_else(|| "No system clipboard utility available".into()))
}

#[cfg(not(any(unix, windows)))]
pub fn set(_text: &str) -> Result<(), String> {
    Err("System clipboard is not supported on this platform".into())
}

#[cfg(not(any(unix, windows)))]
pub fn get() -> Result<String, String> {
    Err("System clipboard is not supported on this platform".into())
}
