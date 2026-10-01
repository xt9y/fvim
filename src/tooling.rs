use crate::{
    config::{Server, ToolSettings},
    diagnostics::Diagnostic,
    syntax::Span,
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    io::{self, BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver, Sender},
    thread,
};

pub fn uri(path: &Path) -> String {
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| {
        if path.is_absolute() {
            path.to_owned()
        } else {
            std::env::current_dir().unwrap_or_default().join(path)
        }
    });
    let raw = path.to_string_lossy().replace('\\', "/");
    let raw = raw.strip_prefix("//?/").unwrap_or(&raw);
    let mut out = if raw.starts_with('/') {
        "file://".to_owned()
    } else {
        "file:///".to_owned()
    };
    for b in raw.bytes() {
        if b.is_ascii_alphanumeric() || b"/:._-~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}
pub fn from_uri(uri: &str) -> Option<PathBuf> {
    let raw = uri.strip_prefix("file://")?;
    let raw = if cfg!(windows) {
        raw.strip_prefix('/').unwrap_or(raw)
    } else {
        raw
    };
    let mut bytes = vec![];
    let mut i = 0;
    while i < raw.len() {
        if raw.as_bytes()[i] == b'%' {
            bytes.push(u8::from_str_radix(raw.get(i + 1..i + 3)?, 16).ok()?);
            i += 3;
        } else {
            bytes.push(raw.as_bytes()[i]);
            i += 1;
        }
    }
    String::from_utf8(bytes).ok().map(PathBuf::from)
}
pub fn utf16_col(line: &str, units: usize) -> usize {
    let mut n = 0;
    let mut col = 0;
    for ch in line.chars() {
        if n + ch.len_utf16() > units {
            break;
        }
        n += ch.len_utf16();
        col += 1;
    }
    col
}
pub fn to_utf16(line: &str, col: usize) -> usize {
    line.chars().take(col).map(char::len_utf16).sum()
}
fn write_rpc(out: &mut impl Write, value: &Value) -> io::Result<()> {
    let bytes = serde_json::to_vec(value)?;
    write!(out, "Content-Length: {}\r\n\r\n", bytes.len())?;
    out.write_all(&bytes)?;
    out.flush()
}
fn read_rpc(input: &mut impl BufRead) -> io::Result<Value> {
    let mut length = None;
    let mut header = String::new();
    let mut total = 0;
    loop {
        header.clear();
        if input.read_line(&mut header)? == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        total += header.len();
        if total > 8192 {
            return Err(io::Error::other("LSP headers too large"));
        }
        if header.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            if name.eq_ignore_ascii_case("Content-Length") {
                length = Some(value.trim().parse::<usize>().map_err(io::Error::other)?);
            }
        }
    }
    let length = length
        .filter(|n| *n <= 16 * 1024 * 1024)
        .ok_or_else(|| io::Error::other("Invalid LSP message size"))?;
    let mut bytes = vec![0; length];
    input.read_exact(&mut bytes)?;
    serde_json::from_slice(&bytes).map_err(io::Error::other)
}

#[derive(Clone)]
pub struct Document {
    pub path: PathBuf,
    pub version: u64,
    pub text: String,
    pub filetype: String,
    pub cursor: Option<(usize, usize)>,
}
pub enum Event {
    Syntax {
        path: PathBuf,
        version: u64,
        spans: Vec<Span>,
        diagnostics: Vec<Diagnostic>,
    },
    Diagnostics {
        path: PathBuf,
        version: Option<u64>,
        source: String,
        items: Vec<Diagnostic>,
    },
    Response {
        method: String,
        path: PathBuf,
        version: u64,
        row: usize,
        col: usize,
        value: Value,
    },
    Status(String),
}
enum Input {
    Document(Document),
    Request {
        path: PathBuf,
        row: usize,
        col: usize,
        method: String,
    },
    Saved(PathBuf),
    Settings(ToolSettings),
    Rpc(String, Value),
    Exit(String, String),
    Stop,
}
pub struct Tools {
    sender: Sender<Input>,
    receiver: Receiver<(u64, Event)>,
    epoch: u64,
}
impl Tools {
    pub fn new(settings: ToolSettings) -> Self {
        let (sender, input) = mpsc::channel();
        let (output, receiver) = mpsc::channel();
        let reader_sender = sender.clone();
        thread::spawn(move || Worker::new(settings, reader_sender, output).run(input));
        Self {
            sender,
            receiver,
            epoch: 0,
        }
    }
    pub fn document(&self, d: Document) {
        let _ = self.sender.send(Input::Document(d));
    }
    pub fn request(&self, path: PathBuf, row: usize, col: usize, method: &str) {
        let _ = self.sender.send(Input::Request {
            path,
            row,
            col,
            method: method.into(),
        });
    }
    pub fn saved(&self, path: PathBuf) {
        let _ = self.sender.send(Input::Saved(path));
    }
    pub fn settings(&mut self, s: ToolSettings) {
        self.epoch = self.epoch.wrapping_add(1);
        let _ = self.sender.send(Input::Settings(s));
    }
    pub fn poll(&self) -> Vec<Event> {
        self.receiver
            .try_iter()
            .filter_map(|(epoch, event)| (epoch == self.epoch).then_some(event))
            .collect()
    }
}
impl Drop for Tools {
    fn drop(&mut self) {
        let _ = self.sender.send(Input::Stop);
    }
}
struct Session {
    child: Child,
    stdin: ChildStdin,
    ready: bool,
    documents: HashMap<String, Document>,
    incremental: bool,
    pending_saves: Vec<String>,
    root: PathBuf,
    name: String,
    requests: HashMap<u64, (String, String, u64, usize, usize)>,
}
impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Session {
    fn send(&mut self, value: Value) -> io::Result<()> {
        write_rpc(&mut self.stdin, &value)
    }
    fn notify(&mut self, method: &str, params: Value) {
        let _ = self.send(json!({"jsonrpc":"2.0","method":method,"params":params}));
    }
    fn update(&mut self, d: Document) {
        let uri = uri(&d.path);
        let old = self.documents.insert(uri.clone(), d.clone());
        if !self.ready {
            return;
        }
        if let Some(old) = old {
            let change = if self.incremental {
                let row = old.text.bytes().filter(|b| *b == b'\n').count();
                let col = old
                    .text
                    .rsplit('\n')
                    .next()
                    .unwrap_or("")
                    .encode_utf16()
                    .count();
                json!({"range":{"start":{"line":0,"character":0},"end":{"line":row,"character":col}},"text":d.text})
            } else {
                json!({"text":d.text})
            };
            self.notify(
                "textDocument/didChange",
                json!({"textDocument":{"uri":uri,"version":d.version},"contentChanges":[change]}),
            );
        } else {
            self.notify("textDocument/didOpen",json!({"textDocument":{"uri":uri,"languageId":d.filetype,"version":d.version,"text":d.text}}));
        }
    }
    fn request(&mut self, id: u64, path: &Path, row: usize, col: usize, method: &str) {
        let uri = uri(path);
        let Some(d) = self.documents.get(&uri) else {
            return;
        };
        if !self.ready {
            return;
        }
        let version = d.version;
        let units = to_utf16(d.text.lines().nth(row).unwrap_or(""), col);
        let mut params =
            json!({"textDocument":{"uri":uri},"position":{"line":row,"character":units}});
        if method == "textDocument/references" {
            params["context"] = json!({"includeDeclaration":true});
        }
        self.requests
            .insert(id, (method.into(), uri, version, row, col));
        let _ = self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}));
    }
}
struct Output {
    epoch: u64,
    sender: Sender<(u64, Event)>,
}
impl Output {
    fn send(&self, event: Event) -> Result<(), mpsc::SendError<(u64, Event)>> {
        self.sender.send((self.epoch, event))
    }
}
struct Worker {
    settings: ToolSettings,
    sender: Sender<Input>,
    output: Output,
    sessions: HashMap<String, Session>,
    failed: Vec<String>,
    id: u64,
    epoch: u64,
}
impl Worker {
    fn new(settings: ToolSettings, sender: Sender<Input>, output: Sender<(u64, Event)>) -> Self {
        Self {
            settings,
            sender,
            output: Output {
                epoch: 0,
                sender: output,
            },
            sessions: HashMap::new(),
            failed: vec![],
            id: 10,
            epoch: 0,
        }
    }
    fn root(path: &Path, server: &Server) -> PathBuf {
        let parent = path.parent().unwrap_or(Path::new("."));
        for dir in parent.ancestors() {
            if server.root_markers.iter().any(|m| dir.join(m).exists()) {
                return dir.to_owned();
            }
        }
        parent.to_owned()
    }
    fn executable(command: &str) -> PathBuf {
        let mut dirs = vec![];
        if let Some(d) = std::env::var_os("XDG_DATA_HOME") {
            dirs.push(PathBuf::from(d).join("nvim/mason/bin"));
        }
        if let Some(h) = std::env::var_os("HOME") {
            dirs.push(PathBuf::from(h).join(".local/share/nvim/mason/bin"));
        }
        if let Some(d) = std::env::var_os("LOCALAPPDATA") {
            dirs.push(PathBuf::from(d).join("nvim-data/mason/bin"));
        }
        for dir in dirs {
            for suffix in if cfg!(windows) {
                vec!["", ".exe", ".cmd", ".bat"]
            } else {
                vec![""]
            } {
                let p = dir.join(format!("{command}{suffix}"));
                if p.is_file() {
                    return p;
                }
            }
        }
        PathBuf::from(command)
    }
    fn start(&mut self, name: &str, server: &Server, root: PathBuf) -> Option<String> {
        let key = format!("{}:{name}:{}", self.epoch, root.display());
        if self.sessions.contains_key(&key) {
            return Some(key);
        }
        if self.failed.contains(&key) {
            return None;
        }
        let mut child = match Command::new(Self::executable(&server.command[0]))
            .args(&server.command[1..])
            .current_dir(&root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                self.failed.push(key);
                let _ = self.output.send(Event::Status(format!("{name}: {e}")));
                return None;
            }
        };
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let sender = self.sender.clone();
        let reader_key = key.clone();
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                match read_rpc(&mut reader) {
                    Ok(value) => {
                        if sender.send(Input::Rpc(reader_key.clone(), value)).is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        let _ = sender.send(Input::Exit(reader_key, e.to_string()));
                        break;
                    }
                }
            }
        });
        let mut session = Session {
            child,
            stdin,
            ready: false,
            documents: HashMap::new(),
            incremental: false,
            pending_saves: vec![],
            root: root.clone(),
            name: name.into(),
            requests: HashMap::new(),
        };
        let _=session.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"processId":std::process::id(),"rootUri":uri(&root),"workspaceFolders":[{"uri":uri(&root),"name":root.file_name().unwrap_or_default().to_string_lossy()}],"capabilities":{"general":{"positionEncodings":["utf-16"]},"textDocument":{"publishDiagnostics":{"versionSupport":true},"completion":{"completionItem":{"snippetSupport":false}}}}}}));
        self.sessions.insert(key.clone(), session);
        Some(key)
    }
    fn run(&mut self, input: Receiver<Input>) {
        while let Ok(event) = input.recv() {
            match event {
                Input::Stop => break,
                Input::Settings(s) => {
                    self.epoch = self.epoch.wrapping_add(1);
                    self.output.epoch = self.epoch;
                    self.settings = s;
                    self.sessions.clear();
                    self.failed.clear();
                }
                Input::Document(d) => {
                    let enabled = self.settings.syntax
                        && (self.settings.languages.contains(&d.filetype) || d.filetype == "llvm");
                    let (spans, diagnostics) =
                        crate::syntax::analyze(&d.path, &d.filetype, &d.text, enabled);
                    let _ = self.output.send(Event::Syntax {
                        path: d.path.clone(),
                        version: d.version,
                        spans,
                        diagnostics,
                    });
                    let mut configs: Vec<_> = self
                        .settings
                        .servers
                        .iter()
                        .filter(|(_, s)| s.enabled && s.filetypes.contains(&d.filetype))
                        .map(|(n, s)| (n.clone(), s.clone()))
                        .collect();
                    configs.sort_by(|a, b| a.0.cmp(&b.0));
                    for (name, server) in configs {
                        let root = Self::root(&d.path, &server);
                        if let Some(key) = self.start(&name, &server, root) {
                            let session = self.sessions.get_mut(&key).unwrap();
                            session.update(d.clone());
                            if self.settings.completion {
                                if let Some((row, col)) = d.cursor {
                                    self.id += 1;
                                    session.request(
                                        self.id,
                                        &d.path,
                                        row,
                                        col,
                                        "textDocument/completion",
                                    );
                                }
                            }
                        }
                    }
                }
                Input::Saved(path) => {
                    let uri = uri(&path);
                    for session in self.sessions.values_mut() {
                        if session.documents.contains_key(&uri) && !session.ready {
                            if !session.pending_saves.contains(&uri) {
                                session.pending_saves.push(uri.clone());
                            }
                        } else if session.ready && session.documents.contains_key(&uri) {
                            session.notify(
                                "textDocument/didSave",
                                json!({"textDocument":{"uri":uri}}),
                            );
                        }
                    }
                }
                Input::Request {
                    path,
                    row,
                    col,
                    method,
                } => {
                    for session in self.sessions.values_mut() {
                        self.id += 1;
                        session.request(self.id, &path, row, col, &method);
                    }
                }
                Input::Exit(key, error) => {
                    if let Some(session) = self.sessions.remove(&key) {
                        let _ = self
                            .output
                            .send(Event::Status(format!("{} exited: {error}", session.name)));
                        self.failed.push(key);
                    }
                }
                Input::Rpc(key, value) => {
                    let Some(session) = self.sessions.get_mut(&key) else {
                        continue;
                    };
                    if let Some(method) = value["method"].as_str() {
                        if method == "textDocument/publishDiagnostics" {
                            let params = &value["params"];
                            let Some(uri) = params["uri"].as_str() else {
                                continue;
                            };
                            let Some(path) = from_uri(uri) else {
                                continue;
                            };
                            let version = params["version"].as_u64();
                            let doc = session.documents.get(uri);
                            if version.is_some_and(|v| doc.is_some_and(|d| v < d.version)) {
                                continue;
                            }
                            let disk;
                            let text = if let Some(doc) = doc {
                                doc.text.as_str()
                            } else {
                                disk = std::fs::read_to_string(&path).unwrap_or_default();
                                &disk
                            };
                            let lines: Vec<_> = text.split('\n').collect();
                            let items = params["diagnostics"]
                                .as_array()
                                .map(|items| {
                                    items
                                        .iter()
                                        .filter_map(|item| {
                                            let start = &item["range"]["start"];
                                            let end = &item["range"]["end"];
                                            let row = start["line"].as_u64()? as usize;
                                            let end_row = end["line"].as_u64()? as usize;
                                            let col = utf16_col(
                                                lines.get(row).copied().unwrap_or(""),
                                                start["character"].as_u64()? as usize,
                                            );
                                            let end_col = utf16_col(
                                                lines.get(end_row).copied().unwrap_or(""),
                                                end["character"].as_u64()? as usize,
                                            );
                                            Some(Diagnostic {
                                                path: Some(path.clone()),
                                                row,
                                                col,
                                                end_row,
                                                end_col,
                                                severity: item["severity"]
                                                    .as_u64()
                                                    .unwrap_or(1)
                                                    .clamp(1, 4)
                                                    as u8,
                                                source: item["source"]
                                                    .as_str()
                                                    .unwrap_or(&session.name)
                                                    .into(),
                                                message: item["message"].as_str()?.into(),
                                            })
                                        })
                                        .collect()
                                })
                                .unwrap_or_default();
                            let _ = self.output.send(Event::Diagnostics {
                                path,
                                version,
                                source: session.name.clone(),
                                items,
                            });
                        } else if let Some(id) = value.get("id") {
                            let result = match method {
                                "workspace/configuration" => Value::Array(
                                    value["params"]["items"]
                                        .as_array()
                                        .map(|items| items.iter().map(|_| json!({})).collect())
                                        .unwrap_or_default(),
                                ),
                                "workspace/workspaceFolders" => {
                                    json!([{"uri":uri(&session.root),"name":session.root.file_name().unwrap_or_default().to_string_lossy()}])
                                }
                                _ => Value::Null,
                            };
                            let _ = session.send(json!({"jsonrpc":"2.0","id":id,"result":result}));
                        }
                    } else if value["id"] == 1 {
                        if value.get("error").is_some() {
                            let _ = self.output.send(Event::Status(format!(
                                "{} initialize: {}",
                                session.name, value["error"]
                            )));
                            continue;
                        }
                        session.ready = true;
                        let sync = &value["result"]["capabilities"]["textDocumentSync"];
                        session.incremental =
                            sync.as_u64() == Some(2) || sync["change"].as_u64() == Some(2);
                        session.notify("initialized", json!({}));
                        let documents = std::mem::take(&mut session.documents);
                        for d in documents.into_values() {
                            session.update(d.clone());
                            if self.settings.completion {
                                if let Some((row, col)) = d.cursor {
                                    self.id += 1;
                                    session.request(
                                        self.id,
                                        &d.path,
                                        row,
                                        col,
                                        "textDocument/completion",
                                    );
                                }
                            }
                        }
                        let _ = self
                            .output
                            .send(Event::Status(format!("{} attached", session.name)));
                        for uri in std::mem::take(&mut session.pending_saves) {
                            session.notify(
                                "textDocument/didSave",
                                json!({"textDocument":{"uri":uri}}),
                            );
                        }
                    } else if let Some(id) = value["id"].as_u64() {
                        if let Some((method, uri, version, row, col)) = session.requests.remove(&id)
                        {
                            if let Some(path) = from_uri(&uri) {
                                if value.get("error").is_none() {
                                    let _ = self.output.send(Event::Response {
                                        method,
                                        path,
                                        version,
                                        row,
                                        col,
                                        value: value["result"].clone(),
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn json_rpc_framing_counts_utf8_bytes_and_accepts_extra_headers() {
        let value = serde_json::json!({"message":"界"});
        let mut bytes = Vec::new();
        write_rpc(&mut bytes, &value).unwrap();
        assert_eq!(
            read_rpc(&mut std::io::BufReader::new(&bytes[..])).unwrap(),
            value
        );
        assert!(read_rpc(&mut std::io::BufReader::new(
            &b"Content-Length: 999999999\r\n\r\n"[..]
        ))
        .is_err());
    }
    #[test]
    fn settings_reload_discards_queued_events_from_previous_generation() {
        let (sender, _input) = mpsc::channel();
        let (output, receiver) = mpsc::channel();
        let mut tools = Tools {
            sender,
            receiver,
            epoch: 0,
        };
        output.send((0, Event::Status("old".into()))).unwrap();
        tools.settings(crate::config::Settings::default().tooling);
        output.send((1, Event::Status("new".into()))).unwrap();
        let events = tools.poll();
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], Event::Status(s) if s=="new"));
    }
    #[test]
    fn file_uris_roundtrip_spaces_and_unicode_and_utf16_positions_convert() {
        let p = std::env::temp_dir().join("wide 界 name.c");
        assert_eq!(from_uri(&uri(&p)).unwrap(), p);
        assert_eq!(utf16_col("a😀界z", 3), 2);
        assert_eq!(to_utf16("a😀界z", 2), 3);
    }
}
