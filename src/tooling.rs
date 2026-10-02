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
    time::{Duration, Instant, SystemTime},
};

pub fn uri(path: &Path) -> String {
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        std::fs::canonicalize(path)
            .unwrap_or_else(|_| std::env::current_dir().unwrap_or_default().join(path))
    };
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

fn lsp_position(text: &str, byte: usize) -> (usize, usize) {
    let prefix = &text[..byte.min(text.len())];
    let row = prefix.bytes().filter(|byte| *byte == b'\n').count();
    let col = prefix
        .rsplit('\n')
        .next()
        .unwrap_or("")
        .encode_utf16()
        .count();
    (row, col)
}

fn incremental_change(old: &str, new: &str) -> Value {
    if old == new {
        let (row, character) = lsp_position(old, old.len());
        return json!({
            "range": {
                "start": {"line": row, "character": character},
                "end": {"line": row, "character": character}
            },
            "text": ""
        });
    }

    let mut prefix = 0usize;
    for (left, right) in old.chars().zip(new.chars()) {
        if left != right {
            break;
        }
        prefix += left.len_utf8();
    }
    let old_tail = &old[prefix..];
    let new_tail = &new[prefix..];
    let mut suffix = 0usize;
    for (left, right) in old_tail.chars().rev().zip(new_tail.chars().rev()) {
        if left != right {
            break;
        }
        suffix += left.len_utf8();
    }
    let old_end = old.len() - suffix;
    let new_end = new.len() - suffix;
    let (start_row, start_col) = lsp_position(old, prefix);
    let (end_row, end_col) = lsp_position(old, old_end);
    json!({
        "range": {
            "start": {"line": start_row, "character": start_col},
            "end": {"line": end_row, "character": end_col}
        },
        "text": &new[prefix..new_end]
    })
}
pub fn compilation_database(path: &Path, root: &Path, explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(dir) = explicit {
        let dir = if dir.is_absolute() {
            dir.to_owned()
        } else {
            root.join(dir)
        };
        return Some(dir);
    }
    for dir in path.parent()?.ancestors() {
        if dir.join("compile_commands.json").is_file() {
            return Some(dir.to_owned());
        }
        if dir == root {
            break;
        }
    }
    let mut dirs: Vec<_> = ["build", ".build", "out", "target"]
        .iter()
        .map(|name| (root.join(name), 0))
        .collect();
    let mut visited = 0;
    while !dirs.is_empty() && visited < 256 {
        let (dir, depth) = dirs.remove(0);
        visited += 1;
        if dir.join("compile_commands.json").is_file() {
            return Some(dir);
        }
        if depth < 2 {
            if let Ok(entries) = std::fs::read_dir(&dir) {
                let mut children: Vec<_> = entries
                    .flatten()
                    .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                    .map(|entry| entry.path())
                    .collect();
                children.sort();
                dirs.extend(
                    children
                        .into_iter()
                        .take(256 - dirs.len().min(256))
                        .map(|p| (p, depth + 1)),
                );
            }
        }
    }
    None
}
fn semantic_spans(
    text: &str,
    result: &Value,
    legend: &[String],
    modifiers: &[String],
) -> Vec<Span> {
    let Some(data) = result["data"].as_array() else {
        return vec![];
    };
    let lines: Vec<_> = text.split('\n').collect();
    let mut spans = vec![];
    let mut row = 0usize;
    let mut units = 0usize;
    for token in data.as_chunks::<5>().0 {
        let values: Option<Vec<_>> = token.iter().map(Value::as_u64).collect();
        let Some(values) = values else {
            continue;
        };
        let Some(next_row) = row.checked_add(values[0] as usize) else {
            continue;
        };
        row = next_row;
        let Some(next_units) =
            (if values[0] == 0 { units } else { 0 }).checked_add(values[1] as usize)
        else {
            continue;
        };
        units = next_units;
        let Some(line) = lines.get(row) else {
            continue;
        };
        let Some(end) = units
            .checked_add(values[2] as usize)
            .filter(|end| *end <= line.encode_utf16().count())
        else {
            continue;
        };
        let Some(kind) = legend.get(values[3] as usize) else {
            continue;
        };
        let readonly = modifiers
            .iter()
            .position(|name| name == "readonly")
            .is_some_and(|bit| bit < 64 && values[4] & (1u64 << bit) != 0);
        let group = match kind.as_str() {
            "namespace" | "type" | "class" | "enum" | "interface" | "struct" | "typeParameter" => {
                "Type"
            }
            "function" | "method" => "Function",
            "property" | "event" => "Property",
            "enumMember" | "macro" => "Constant",
            "variable" | "parameter" if readonly => "Constant",
            "variable" | "parameter" => "Variable",
            "keyword" | "modifier" | "operator" | "decorator" => "Keyword",
            "comment" => "Comment",
            "string" | "regexp" => "String",
            "number" => "Number",
            _ => continue,
        };
        let start = utf16_col(line, units);
        let end = utf16_col(line, end);
        if start < end {
            spans.push(Span {
                row,
                start,
                end,
                group,
            });
        }
    }
    spans
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
fn keep_latest(documents: &mut HashMap<String, Document>, document: Document) {
    let key = uri(&document.path);
    if documents
        .get(&key)
        .is_none_or(|old| old.version <= document.version)
    {
        documents.insert(key, document);
    }
}
fn database_stamp(dir: &Option<PathBuf>) -> Option<(SystemTime, u64)> {
    let metadata = std::fs::metadata(dir.as_ref()?.join("compile_commands.json")).ok()?;
    Some((metadata.modified().ok()?, metadata.len()))
}
pub enum Event {
    Semantic {
        path: PathBuf,
        version: u64,
        spans: Vec<Span>,
    },
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
    ProjectBuilt(PathBuf),
    Rpc(String, Value),
    Exit(String, String),
    Stop,
}

type SyntaxTask = (u64, Document, bool);

fn syntax_worker(input: Receiver<SyntaxTask>, output: Sender<(u64, Event)>) {
    while let Ok(first) = input.recv() {
        let mut pending = HashMap::new();
        pending.insert(first.1.path.clone(), first);
        while let Ok(task) = input.try_recv() {
            pending.insert(task.1.path.clone(), task);
        }
        for (_, (epoch, document, enabled)) in pending {
            let (spans, diagnostics) =
                crate::syntax::analyze(&document.path, &document.filetype, &document.text, enabled);
            if output
                .send((
                    epoch,
                    Event::Syntax {
                        path: document.path,
                        version: document.version,
                        spans,
                        diagnostics,
                    },
                ))
                .is_err()
            {
                return;
            }
        }
    }
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
        let (syntax, syntax_input) = mpsc::channel();
        let syntax_output = output.clone();
        thread::Builder::new()
            .name("fvim-syntax".into())
            .spawn(move || syntax_worker(syntax_input, syntax_output))
            .expect("spawn syntax worker");
        let reader_sender = sender.clone();
        thread::Builder::new()
            .name("fvim-tooling".into())
            .spawn(move || Worker::new(settings, reader_sender, output, syntax).run(input))
            .expect("spawn tooling worker");
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
    pub fn project_built(&self, root: PathBuf) {
        let _ = self.sender.send(Input::ProjectBuilt(root));
    }
    pub fn saved(&self, path: PathBuf) {
        let _ = self.sender.send(Input::Saved(path));
    }
    pub fn settings(&mut self, s: ToolSettings) {
        self.epoch = self.epoch.wrapping_add(1);
        let _ = self.sender.send(Input::Settings(s));
    }
    pub fn try_event(&self) -> Option<Event> {
        loop {
            match self.receiver.try_recv() {
                Ok((epoch, event)) if epoch == self.epoch => return Some(event),
                Ok(_) => continue,
                Err(_) => return None,
            }
        }
    }

    #[cfg(test)]
    pub fn poll(&self) -> Vec<Event> {
        std::iter::from_fn(|| self.try_event()).collect()
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
    signature: String,
    database: Option<PathBuf>,
    database_stamp: Option<(SystemTime, u64)>,
    semantic_types: Vec<String>,
    semantic_modifiers: Vec<String>,
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
        if self
            .documents
            .get(&uri)
            .is_some_and(|old| old.version > d.version)
        {
            return;
        }
        let old = self.documents.insert(uri.clone(), d.clone());
        if !self.ready {
            return;
        }
        if let Some(old) = old {
            if old.version == d.version && old.text == d.text {
                return;
            }
            let change = if self.incremental {
                incremental_change(&old.text, &d.text)
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
    fn semantic(&mut self, id: u64, path: &Path) {
        if !self.semantic_types.is_empty() {
            self.request(id, path, 0, 0, "textDocument/semanticTokens/full");
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
        if method == "textDocument/semanticTokens/full" {
            params = json!({"textDocument":{"uri":uri}});
        }
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
    syntax: Sender<SyntaxTask>,
    sessions: HashMap<String, Session>,
    failed: Vec<String>,
    id: u64,
    epoch: u64,
}
impl Worker {
    fn new(
        settings: ToolSettings,
        sender: Sender<Input>,
        output: Sender<(u64, Event)>,
        syntax: Sender<SyntaxTask>,
    ) -> Self {
        Self {
            settings,
            sender,
            output: Output {
                epoch: 0,
                sender: output,
            },
            syntax,
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
    fn start(
        &mut self,
        name: &str,
        server: &Server,
        root: PathBuf,
        database: Option<PathBuf>,
    ) -> Option<String> {
        let signature = format!(
            "{}:{name}:{}:{}",
            self.epoch,
            root.display(),
            database
                .as_ref()
                .map_or(String::new(), |p| p.display().to_string())
        );
        if let Some((key, _)) = self
            .sessions
            .iter()
            .find(|(_, session)| session.signature == signature)
        {
            return Some(key.clone());
        }
        if self.failed.contains(&signature) {
            return None;
        }
        // A newly discovered database replaces the existing project session.
        // Preserve every open document, keeping its newest unsaved version.
        let superseded: Vec<_> = self
            .sessions
            .iter()
            .filter(|(_, s)| s.name == name && s.root == root)
            .map(|(key, _)| key.clone())
            .collect();
        let mut documents = HashMap::new();
        for old in superseded {
            if let Some(session) = self.sessions.remove(&old) {
                for document in session.documents.values().cloned() {
                    keep_latest(&mut documents, document);
                }
            }
        }
        self.id += 1;
        let key = format!("{signature}:{}", self.id);
        let mut command = server.command.clone();
        if name == "clangd"
            && !command
                .iter()
                .any(|arg| arg.starts_with("--compile-commands-dir"))
        {
            if let Some(dir) = &database {
                command.push(format!("--compile-commands-dir={}", dir.display()));
            }
        }
        let mut child = match Command::new(Self::executable(&command[0]))
            .args(&command[1..])
            .current_dir(&root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                self.failed.push(signature);
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
            documents,
            incremental: false,
            pending_saves: vec![],
            signature,
            database_stamp: database_stamp(&database),
            database,
            semantic_types: vec![],
            semantic_modifiers: vec![],
            root: root.clone(),
            name: name.into(),
            requests: HashMap::new(),
        };
        let _=session.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"processId":std::process::id(),"rootUri":uri(&root),"workspaceFolders":[{"uri":uri(&root),"name":root.file_name().unwrap_or_default().to_string_lossy()}],"capabilities":{"workspace":{"semanticTokens":{"refreshSupport":true}},"general":{"positionEncodings":["utf-16"]},"textDocument":{"publishDiagnostics":{"versionSupport":true},"semanticTokens":{"requests":{"full":true,"range":false},"tokenTypes":["namespace","type","class","enum","interface","struct","typeParameter","parameter","variable","property","enumMember","event","function","method","macro","keyword","modifier","comment","string","number","regexp","operator","decorator"],"tokenModifiers":["declaration","definition","readonly","static","deprecated","abstract","async","modification","documentation","defaultLibrary"],"formats":["relative"],"overlappingTokenSupport":false,"multilineTokenSupport":false},"completion":{"completionItem":{"snippetSupport":true}}}}}}));
        self.sessions.insert(key.clone(), session);
        Some(key)
    }
    fn restart_project(&mut self, root: &Path) {
        let keys: Vec<_> = self
            .sessions
            .iter()
            .filter(|(_, s)| {
                s.name == "clangd" && (s.root.starts_with(root) || root.starts_with(&s.root))
            })
            .map(|(key, _)| key.clone())
            .collect();
        let mut documents = HashMap::new();
        for key in keys {
            if let Some(session) = self.sessions.remove(&key) {
                for document in session.documents.values().cloned() {
                    keep_latest(&mut documents, document);
                }
            }
        }
        self.failed.retain(|key| !key.contains(":clangd:"));
        for document in documents.into_values() {
            let _ = self.sender.send(Input::Document(document));
        }
    }
    fn refresh_databases(&mut self) {
        let Some(server) = self.settings.servers.get("clangd") else {
            return;
        };
        let roots: Vec<_> = self
            .sessions
            .values()
            .filter(|s| s.name == "clangd")
            .filter_map(|s| {
                let document = s.documents.values().next()?;
                let database = compilation_database(
                    &document.path,
                    &s.root,
                    server.compile_commands_dir.as_deref(),
                );
                (database != s.database || database_stamp(&database) != s.database_stamp)
                    .then(|| s.root.clone())
            })
            .collect();
        for root in roots {
            self.restart_project(&root);
        }
    }
    fn run(&mut self, input: Receiver<Input>) {
        let mut checked = Instant::now();
        loop {
            if checked.elapsed() >= Duration::from_secs(1) {
                self.refresh_databases();
                checked = Instant::now();
            }
            let event = match input.recv_timeout(Duration::from_millis(250)) {
                Ok(event) => event,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            };
            match event {
                Input::Stop => break,
                Input::Settings(s) => {
                    self.epoch = self.epoch.wrapping_add(1);
                    self.output.epoch = self.epoch;
                    self.settings = s;
                    self.sessions.clear();
                    self.failed.clear();
                }
                Input::ProjectBuilt(root) => {
                    self.restart_project(&root);
                }
                Input::Document(d) => {
                    let enabled = self.settings.syntax
                        && (self.settings.languages.contains(&d.filetype) || d.filetype == "llvm");
                    let _ = self.syntax.send((self.epoch, d.clone(), enabled));
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
                        let database = (name == "clangd")
                            .then(|| {
                                compilation_database(
                                    &d.path,
                                    &root,
                                    server.compile_commands_dir.as_deref(),
                                )
                            })
                            .flatten();
                        if let Some(key) = self.start(&name, &server, root, database) {
                            let session = self.sessions.get_mut(&key).unwrap();
                            session.update(d.clone());
                            self.id += 1;
                            session.semantic(self.id, &d.path);
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
                        self.failed.push(session.signature.clone());
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
                            if method == "workspace/semanticTokens/refresh" {
                                let paths: Vec<_> =
                                    session.documents.values().map(|d| d.path.clone()).collect();
                                for path in paths {
                                    self.id += 1;
                                    session.semantic(self.id, &path);
                                }
                            }
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
                        let semantic = &value["result"]["capabilities"]["semanticTokensProvider"];
                        if semantic["full"] == true || semantic["full"].is_object() {
                            session.semantic_types =
                                serde_json::from_value(semantic["legend"]["tokenTypes"].clone())
                                    .unwrap_or_default();
                            session.semantic_modifiers = serde_json::from_value(
                                semantic["legend"]["tokenModifiers"].clone(),
                            )
                            .unwrap_or_default();
                        }
                        let sync = &value["result"]["capabilities"]["textDocumentSync"];
                        session.incremental =
                            sync.as_u64() == Some(2) || sync["change"].as_u64() == Some(2);
                        session.notify("initialized", json!({}));
                        let documents = std::mem::take(&mut session.documents);
                        for d in documents.into_values() {
                            session.update(d.clone());
                            self.id += 1;
                            session.semantic(self.id, &d.path);
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
                                    if method == "textDocument/semanticTokens/full" {
                                        if let Some(d) = session
                                            .documents
                                            .get(&uri)
                                            .filter(|d| d.version == version)
                                        {
                                            let spans = semantic_spans(
                                                &d.text,
                                                &value["result"],
                                                &session.semantic_types,
                                                &session.semantic_modifiers,
                                            );
                                            let _ = self.output.send(Event::Semantic {
                                                path,
                                                version,
                                                spans,
                                            });
                                        }
                                        continue;
                                    }
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
    fn incremental_change_sends_only_the_changed_utf16_range() {
        assert_eq!(
            incremental_change("😀 value\nnext", "😀 value!\nnext"),
            json!({
                "range": {
                    "start": {"line": 0, "character": 8},
                    "end": {"line": 0, "character": 8}
                },
                "text": "!"
            })
        );
    }

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
    fn real_clangd_reloads_a_generated_database_and_produces_semantic_highlights() {
        let Ok(command) = std::env::var("FVIM_TEST_CLANGD") else {
            return;
        };
        if command.is_empty() {
            return;
        }
        let root = std::env::temp_dir().join(format!("fvim-real-clangd-{}", std::process::id()));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("include")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let path = root.join("src/main.c");
        let text="#include \"project.h\"\n#ifndef PROJECT_DEFINE\n#error missing compile command macro\n#endif\nProjectType run(ProjectType input) { return input; }\n";
        std::fs::write(&path, text).unwrap();
        std::fs::write(root.join("include/project.h"), "typedef int ProjectType;\n").unwrap();
        let mut settings = crate::config::Settings::default().tooling;
        settings.servers.retain(|name, _| name == "clangd");
        settings.servers.get_mut("clangd").unwrap().command =
            vec![command, "--background-index".into()];
        let tools = Tools::new(settings);
        tools.document(Document {
            path: path.clone(),
            version: 1,
            text: text.into(),
            filetype: "c".into(),
            cursor: None,
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let mut missing = false;
        while std::time::Instant::now() < deadline && !missing {
            for event in tools.poll() {
                if let Event::Diagnostics {
                    path: got, items, ..
                } = event
                {
                    if got == path {
                        missing = items.iter().any(|d| d.severity == 1);
                    }
                }
            }
            thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            missing,
            "Real clangd did not report absent include paths/macros"
        );
        std::fs::create_dir_all(root.join("build/debug")).unwrap();
        let database = json!([{"directory":root,"file":path,"arguments":["clang","-x","c","-std=c11","-I",root.join("include"),"-DPROJECT_DEFINE=1","-c",path]}]);
        std::fs::write(
            root.join("build/debug/compile_commands.json"),
            database.to_string(),
        )
        .unwrap();
        // The run stage can remain interactive: discovery must refresh without
        // waiting for the build command to exit or for the source to change.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let mut clean = false;
        let mut highlighted = false;
        let mut messages = vec![];
        while std::time::Instant::now() < deadline && !(clean && highlighted) {
            for event in tools.poll() {
                match event {
                    Event::Diagnostics {
                        path: got, items, ..
                    } if got == path => {
                        clean = items.is_empty();
                        messages = items.iter().map(|d| d.message.clone()).collect();
                    }
                    Event::Semantic {
                        path: got, spans, ..
                    } if got == path => {
                        highlighted = spans
                            .iter()
                            .any(|span| span.row == 4 && span.start == 0 && span.group == "Type");
                    }
                    _ => {}
                }
            }
            thread::sleep(std::time::Duration::from_millis(20));
        }
        drop(tools);
        std::fs::remove_dir_all(root).unwrap();
        assert!(clean && highlighted,"Real clangd did not consume generated compilation database: clean={clean}, highlighted={highlighted}, diagnostics={messages:?}");
    }
    #[test]
    fn restart_replay_keeps_the_newest_document_in_any_order() {
        let document = |version| Document {
            path: PathBuf::from("latest.c"),
            version,
            text: format!("version {version}"),
            filetype: "c".into(),
            cursor: None,
        };
        for versions in [[1, 2, 1], [2, 1, 2]] {
            let mut documents = HashMap::new();
            for version in versions {
                keep_latest(&mut documents, document(version));
            }
            assert_eq!(documents.len(), 1);
            assert_eq!(documents.values().next().unwrap().text, "version 2");
        }
    }
    #[test]
    fn semantic_tokens_decode_relative_positions_utf16_and_readonly_variables() {
        let text = "😀 Thing value;\nThing call();";
        let spans = semantic_spans(
            text,
            &json!({"data":[0,3,5,0,0,0,6,5,1,1,1,6,4,2,0]}),
            &["type".into(), "variable".into(), "function".into()],
            &["readonly".into()],
        );
        assert_eq!(
            (spans[0].row, spans[0].start, spans[0].end, spans[0].group),
            (0, 2, 7, "Type")
        );
        assert_eq!(spans[1].group, "Constant");
        assert_eq!((spans[2].row, spans[2].start, spans[2].end), (1, 6, 10));
    }
    #[test]
    fn compilation_database_discovery_prefers_source_ancestors_then_build_directories() {
        let root = std::env::temp_dir().join(format!("fvim-cdb-{}", std::process::id()));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("build/debug")).unwrap();
        std::fs::write(root.join("build/debug/compile_commands.json"), "[]").unwrap();
        assert_eq!(
            compilation_database(&root.join("src/test.c"), &root, None),
            Some(root.join("build/debug"))
        );
        std::fs::write(root.join("compile_commands.json"), "[]").unwrap();
        assert_eq!(
            compilation_database(&root.join("src/test.c"), &root, None),
            Some(root.clone())
        );
        assert_eq!(
            compilation_database(
                &root.join("src/test.c"),
                &root,
                Some(Path::new("build/debug"))
            ),
            Some(root.join("build/debug"))
        );
        std::fs::remove_dir_all(root).unwrap();
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
