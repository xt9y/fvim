use crate::config::Settings;
#[cfg(windows)]
use std::time::{Duration, Instant};
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver},
    thread,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub path: Option<PathBuf>,
    pub row: usize,
    pub col: usize,
    pub end_row: usize,
    pub end_col: usize,
    pub severity: u8,
    pub source: String,
    pub message: String,
}
impl Diagnostic {
    pub fn group(&self) -> &'static str {
        match self.severity {
            1 => "DiagnosticError",
            2 => "DiagnosticWarn",
            3 => "DiagnosticInfo",
            _ => "DiagnosticHint",
        }
    }
    pub fn sign(&self) -> &'static str {
        match self.severity {
            1 => "E",
            2 => "W",
            3 => "I",
            _ => "H",
        }
    }
    pub fn contains(&self, row: usize, col: usize) -> bool {
        let end_col = if self.end_row == self.row {
            self.end_col.max(self.col + 1)
        } else {
            self.end_col
        };
        (row, col) >= (self.row, self.col) && (row, col) < (self.end_row, end_col)
    }
    pub fn label(&self) -> String {
        format!(
            "{} [{}] {}  {}:{}:{}",
            self.sign(),
            self.source,
            self.message,
            self.path
                .as_ref()
                .map_or("[Build]".into(), |p| p.display().to_string()),
            self.row + 1,
            self.col + 1,
        )
    }
}

pub fn plain_output(text: &str) -> String {
    use std::sync::OnceLock;
    static ANSI: OnceLock<regex::Regex> = OnceLock::new();
    ANSI.get_or_init(|| {
        regex::Regex::new(r"\x1b\[[0-?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)").unwrap()
    })
    .replace_all(text, "")
    .into_owned()
}

pub fn compiler(root: &Path, text: &str) -> Vec<Diagnostic> {
    use std::sync::OnceLock;
    static ANSI: OnceLock<regex::Regex> = OnceLock::new();
    static GCC: OnceLock<regex::Regex> = OnceLock::new();
    static MSVC: OnceLock<regex::Regex> = OnceLock::new();
    let clean = ANSI
        .get_or_init(|| regex::Regex::new(r"\x1b\[[0-9;]*[A-Za-z]").unwrap())
        .replace_all(text, "");
    let gcc = GCC.get_or_init(|| {
        regex::Regex::new(
            r"^(.+?):(\d+)(?::(\d+))?:\s*(fatal error|error|warning|note|hint):\s*(.*)$",
        )
        .unwrap()
    });
    let msvc = MSVC.get_or_init(|| {
        regex::Regex::new(
            r"(?i)^(.+)\((\d+)(?:[:,](\d+))?\):?\s*(fatal error|error|warning|note|hint):?\s*(.*)$",
        )
        .unwrap()
    });
    clean
        .lines()
        .filter_map(|line| {
            let Some(c) = gcc.captures(line).or_else(|| msvc.captures(line)) else {
                let line = line.trim();
                let (severity, message) = if let Some(message) = line
                    .strip_prefix("error:")
                    .or_else(|| line.strip_prefix("fatal error:"))
                {
                    (1, message)
                } else if let Some(message) = line.strip_prefix("warning:") {
                    (2, message)
                } else if line.contains("undefined reference") {
                    (1, line)
                } else {
                    return None;
                };
                return Some(Diagnostic {
                    path: None,
                    row: 0,
                    col: 0,
                    end_row: 0,
                    end_col: 1,
                    severity,
                    source: "compiler".into(),
                    message: message.trim().into(),
                });
            };
            let path = PathBuf::from(c[1].trim());
            let path = if path.is_absolute() || c[1].as_bytes().get(1) == Some(&b':') {
                path
            } else {
                root.join(path)
            };
            let row = c[2].parse::<usize>().ok()?.saturating_sub(1);
            let col = c
                .get(3)
                .and_then(|c| c.as_str().parse::<usize>().ok())
                .unwrap_or(1)
                .saturating_sub(1);
            let severity = match c[4].to_ascii_lowercase().as_str() {
                "error" | "fatal error" => 1,
                "warning" => 2,
                "hint" => 4,
                _ => 3,
            };
            Some(Diagnostic {
                path: Some(path),
                row,
                col,
                end_row: row,
                end_col: col + 1,
                severity,
                source: "compiler".into(),
                message: c[5].trim().into(),
            })
        })
        .collect()
}

pub fn build_command(root: &Path, settings: &Settings, args: &str) -> Result<Vec<String>, String> {
    let mut command = if ["Makefile", "makefile", "GNUmakefile"]
        .iter()
        .any(|p| root.join(p).is_file())
    {
        settings.make_command.clone()
    } else {
        settings.fallback_command.clone()
    };
    command.extend(shell_words::split(args).map_err(|e| format!("Invalid build arguments: {e}"))?);
    Ok(command)
}

#[cfg(any(windows, test))]
fn inherited_cursor_output(pending: &mut String, text: &str) -> (String, bool) {
    const QUERY: &str = "\x1b[6n";
    pending.push_str(text);
    if let Some(index) = pending.find(QUERY) {
        pending.replace_range(index..index + QUERY.len(), "");
        return (std::mem::take(pending), true);
    }
    let tail = (1..QUERY.len())
        .rev()
        .find(|&len| pending.ends_with(&QUERY[..len]))
        .unwrap_or(0);
    let keep = pending.split_off(pending.len() - tail);
    let output = std::mem::replace(pending, keep);
    (output, false)
}

pub enum BuildEvent {
    Output(String),
    Finished,
}
pub struct Build {
    pub buffer: usize,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    #[cfg(unix)]
    process_group: Option<libc::pid_t>,
    master: Option<Box<dyn portable_pty::MasterPty + Send>>,
    writer: Option<Box<dyn Write + Send>>,
    parser: vt100::Parser,
    receiver: Receiver<BuildEvent>,
    pub text: String,
    readers: usize,
    exit: Option<String>,
    #[cfg(windows)]
    inherited_cursor: bool,
    #[cfg(windows)]
    cursor_pending: String,
    #[cfg(windows)]
    exit_drain_deadline: Option<Instant>,
}
impl Build {
    pub fn start(root: &Path, command: &[String], buffer: usize) -> Result<Self, String> {
        use portable_pty::{native_pty_system, CommandBuilder, PtySize};
        let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
        let pair = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("Cannot open build terminal: {e}"))?;
        let mut process = CommandBuilder::new(&command[0]);
        process.args(&command[1..]);
        process.cwd(root);
        let child = pair
            .slave
            .spawn_command(process)
            .map_err(|e| format!("Cannot run {}: {e}", command[0]))?;
        drop(pair.slave);
        let pipe = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
        let writer = pair.master.take_writer().map_err(|e| e.to_string())?;
        let (sender, receiver) = mpsc::channel();
        fn reader(pipe: impl std::io::Read + Send + 'static, sender: mpsc::Sender<BuildEvent>) {
            thread::spawn(move || {
                let mut pipe = pipe;
                let mut chunk = [0u8; 4096];
                let mut pending = Vec::new();
                loop {
                    let count = match pipe.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(count) => count,
                    };
                    pending.extend_from_slice(&chunk[..count]);
                    let valid = match std::str::from_utf8(&pending) {
                        Ok(_) => pending.len(),
                        Err(error) if error.error_len().is_none() => error.valid_up_to(),
                        Err(_) => pending.len(),
                    };
                    if valid > 0 {
                        if sender
                            .send(BuildEvent::Output(
                                String::from_utf8_lossy(&pending[..valid]).into_owned(),
                            ))
                            .is_err()
                        {
                            return;
                        }
                        pending.drain(..valid);
                    }
                }
                if !pending.is_empty() {
                    let _ = sender.send(BuildEvent::Output(
                        String::from_utf8_lossy(&pending).into_owned(),
                    ));
                }
                let _ = sender.send(BuildEvent::Finished);
            });
        }
        reader(pipe, sender);
        Ok(Self {
            buffer,
            #[cfg(unix)]
            process_group: child.process_id().map(|id| id as libc::pid_t),
            child,
            master: Some(pair.master),
            writer: Some(writer),
            parser: vt100::Parser::new(rows, cols, 1000),
            receiver,
            text: format!("$ {}\n", command.join(" ")),
            readers: 1,
            exit: None,
            #[cfg(windows)]
            inherited_cursor: true,
            #[cfg(windows)]
            cursor_pending: String::new(),
            #[cfg(windows)]
            exit_drain_deadline: None,
        })
    }
    pub fn poll(&mut self) -> (bool, bool) {
        let mut changed = false;
        while let Ok(event) = self.receiver.try_recv() {
            changed = true;
            match event {
                BuildEvent::Output(text) => {
                    #[cfg(windows)]
                    let text = if self.inherited_cursor {
                        let (text, report) =
                            inherited_cursor_output(&mut self.cursor_pending, &text);
                        if report {
                            // portable-pty asks for the starting cursor on Windows.
                            // This is a fresh build terminal with its cursor at 1,1.
                            let _ = self.input(b"\x1b[1;1R");
                            self.inherited_cursor = false;
                        }
                        text
                    } else {
                        text
                    };
                    self.parser.process(text.as_bytes());
                    self.text.push_str(&text);
                }
                BuildEvent::Finished => {
                    #[cfg(windows)]
                    {
                        self.parser.process(self.cursor_pending.as_bytes());
                        self.text
                            .push_str(&std::mem::take(&mut self.cursor_pending));
                    }
                    self.readers = self.readers.saturating_sub(1);
                }
            }
        }
        if self.exit.is_none() {
            if let Ok(Some(status)) = self.child.try_wait() {
                self.exit = Some(format!("Build {status}"));
                self.writer.take();
                #[cfg(windows)]
                {
                    // ConPTY can report process exit before its output pipe has
                    // delivered the child's final stdout/stderr. Keep the master
                    // alive briefly so the reader thread can drain those bytes.
                    self.exit_drain_deadline = Some(Instant::now() + Duration::from_millis(250));
                }
                changed = true;
            }
        }
        #[cfg(windows)]
        if self.exit.is_some() && self.master.is_some() {
            let drained = self.readers == 0;
            let expired = self
                .exit_drain_deadline
                .is_some_and(|deadline| Instant::now() >= deadline);
            if drained || expired {
                self.master.take();
                self.exit_drain_deadline = None;
            }
        }
        let finished = self.exit.is_some() && self.readers == 0;
        if finished {
            self.text
                .push_str(&format!("\n{}\n", self.exit.as_ref().unwrap()));
        }
        (changed, finished)
    }
    pub fn input(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        if let Some(writer) = &mut self.writer {
            writer.write_all(bytes)?;
            writer.flush()?;
        }
        Ok(())
    }
    pub fn screen(&self) -> &vt100::Screen {
        self.parser.screen()
    }
    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.parser.screen_mut().set_size(rows, cols);
        if let Some(master) = &self.master {
            let _ = master.resize(portable_pty::PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            });
        }
    }
    pub fn cancel(&mut self) {
        #[cfg(unix)]
        {
            if let Some(group) = self
                .master
                .as_ref()
                .and_then(|master| master.process_group_leader())
            {
                unsafe {
                    libc::kill(-group, libc::SIGKILL);
                }
            }
        }
        #[cfg(unix)]
        if let Some(group) = self.process_group {
            unsafe {
                libc::kill(-group, libc::SIGKILL);
            }
        }
        #[cfg(windows)]
        {
            if let Some(id) = self.child.process_id() {
                let _ = std::process::Command::new("taskkill")
                    .args(["/T", "/F", "/PID", &id.to_string()])
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
            }
        }
        let _ = self.child.kill();
    }
}
impl Drop for Build {
    fn drop(&mut self) {
        if self.exit.is_none() || self.readers > 0 {
            self.cancel();
        }
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn conpty_inherited_cursor_query_is_answered_even_across_output_chunks() {
        let mut pending = String::new();
        assert_eq!(
            inherited_cursor_output(&mut pending, "before\x1b["),
            ("before".into(), false)
        );
        assert_eq!(
            inherited_cursor_output(&mut pending, "6nstartup"),
            ("startup".into(), true)
        );
        assert!(pending.is_empty());
    }
    #[test]
    fn diagnostic_messages_remain_visible_before_long_paths() {
        let d = Diagnostic {
            path: Some(PathBuf::from("long-directory/".repeat(20))),
            row: 0,
            col: 0,
            end_row: 0,
            end_col: 1,
            severity: 1,
            source: "lsp".into(),
            message: "visible error".into(),
        };
        assert!(d.label().starts_with("E [lsp] visible error"));
    }
    #[cfg(unix)]
    #[test]
    fn cancel_terminates_background_children_after_the_parent_exits() {
        let command = vec![
            "sh".into(),
            "-c".into(),
            "trap '' HUP TERM; sleep 10 & echo started".into(),
        ];
        let mut build = Build::start(Path::new("."), &command, 0).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while build.exit.is_none() && std::time::Instant::now() < deadline {
            build.poll();
            thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(build.exit.is_some());
        build.cancel();
        while std::time::Instant::now() < deadline {
            if build.poll().1 {
                return;
            }
            thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("Background child kept build terminal open after cancellation");
    }
    #[test]
    fn multiline_ranges_end_at_the_reported_column() {
        let d = Diagnostic {
            path: None,
            row: 0,
            col: 10,
            end_row: 1,
            end_col: 2,
            severity: 1,
            source: "test".into(),
            message: "test".into(),
        };
        assert!(d.contains(1, 1));
        assert!(!d.contains(1, 2));
    }
    #[test]
    fn compiler_locations_parse_gcc_zig_and_msvc_with_spaces_and_windows_drives() {
        let root = std::path::Path::new("project");
        let d=compiler(root,"\x1b[31msrc/test.c:3:5: error: unknown name\x1b[0m\nC:\\work space\\file.cpp(12,7): warning C1234: bad conversion\nfile.zig:4:2: error: expected ';'");
        assert_eq!(d.len(), 3);
        let odin = compiler(root, "src/main.odin(4:6) Error: Undeclared name");
        assert_eq!((odin[0].row, odin[0].col, odin[0].severity), (3, 5, 1));
        assert_eq!((d[0].row, d[0].col, d[0].severity), (2, 4, 1));
        assert_eq!((d[1].row, d[1].col, d[1].severity), (11, 6, 2));
        assert!(d[1]
            .path
            .as_ref()
            .unwrap()
            .to_string_lossy()
            .contains("work space"));
    }
    #[test]
    fn build_command_selects_make_or_c_fallback_and_preserves_quoted_arguments() {
        let root =
            std::env::temp_dir().join(format!("fvim-build-selection-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let s = crate::config::Settings::default();
        assert_eq!(
            build_command(&root, &s, "--flag 'two words'").unwrap(),
            vec!["c", "build", "run", "--flag", "two words"]
        );
        std::fs::write(root.join("Makefile"), "").unwrap();
        assert_eq!(
            build_command(&root, &s, "test").unwrap(),
            vec!["make", "test"]
        );
        assert!(build_command(&root, &s, "'unterminated").is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
