use crate::{config::Settings, workspace::Axis};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, Sender},
};

#[derive(Clone, Debug)]
pub struct Entry {
    pub path: PathBuf,
    pub row: usize,
    pub col: usize,
    pub label: String,
}
struct Results {
    generation: usize,
    entries: Vec<Entry>,
    message: String,
}
pub struct Picker {
    pub grep: bool,
    pub query: String,
    pub entries: Vec<Entry>,
    pub selected: usize,
    pub message: String,
    generation: usize,
    sender: Sender<(usize, String)>,
    receiver: Receiver<Results>,
    pending: Vec<KeyEvent>,
    split_maps: Vec<crate::config::Keymap>,
}

fn paths(root: &Path, settings: &Settings) -> Vec<PathBuf> {
    let mut files = vec![];
    let mut dirs = vec![root.to_owned()];
    let mut visited = 0;
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            visited += 1;
            if visited > settings.picker_max_files.saturating_mul(4) {
                files.sort();
                return files;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            if settings.picker_exclude.contains(&name) {
                continue;
            }
            if kind.is_dir() {
                dirs.push(entry.path());
            } else if kind.is_file() {
                files.push(entry.path());
                if files.len() >= settings.picker_max_files {
                    files.sort();
                    return files;
                }
            }
        }
    }
    files.sort();
    files
}

fn fuzzy(label: &str, query: &str) -> Option<usize> {
    let label = label.to_lowercase();
    let query = query.to_lowercase();
    let mut offset = 0;
    let mut score = 0;
    for ch in query.chars() {
        let n = label[offset..].find(ch)?;
        score += n;
        offset += n + ch.len_utf8();
    }
    Some(score)
}

fn search(
    root: &Path,
    files: &[PathBuf],
    grep: bool,
    query: &str,
    max_bytes: usize,
) -> (Vec<Entry>, String) {
    let mut entries = vec![];
    let regex = if grep && !query.is_empty() {
        match regex::Regex::new(query) {
            Ok(r) => Some(r),
            Err(e) => return (vec![], e.to_string()),
        }
    } else {
        None
    };
    if grep && regex.is_none() {
        return (vec![], "Type a regular expression".into());
    }
    let mut ranked = vec![];
    for path in files {
        let label = path.strip_prefix(root).unwrap_or(path).to_string_lossy();
        if let Some(regex) = &regex {
            if fs::metadata(path).map_or(true, |m| m.len() > max_bytes as u64) {
                continue;
            }
            use std::io::Read;
            let Ok(file) = fs::File::open(path) else {
                continue;
            };
            let mut text = String::new();
            if file
                .take(max_bytes as u64 + 1)
                .read_to_string(&mut text)
                .is_err()
                || text.len() > max_bytes
            {
                continue;
            }
            if text.contains('\0') {
                continue;
            }
            for (row, line) in text.lines().enumerate() {
                if let Some(m) = regex.find(line) {
                    entries.push(Entry {
                        path: path.clone(),
                        row,
                        col: line[..m.start()].chars().count(),
                        label: format!(
                            "{label}:{}:{}: {line}",
                            row + 1,
                            line[..m.start()].chars().count() + 1
                        ),
                    });
                    if entries.len() >= 2000 {
                        return (entries, "First 2000 matches".into());
                    }
                }
            }
        } else if let Some(score) = fuzzy(&label, query) {
            ranked.push((
                score,
                Entry {
                    path: path.clone(),
                    row: 0,
                    col: 0,
                    label: label.into_owned(),
                },
            ));
        }
    }
    if !grep {
        ranked.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.label.cmp(&b.1.label)));
        entries = ranked.into_iter().take(2000).map(|p| p.1).collect();
    }
    let message = format!("{} results", entries.len());
    (entries, message)
}

impl Picker {
    pub fn new(root: &Path, grep: bool, query: &str, settings: &Settings) -> Result<Self, String> {
        let root = root.to_owned();
        let split_maps = settings
            .keymaps
            .iter()
            .filter(|m| m.mode == "n" && matches!(m.action.as_str(), "split" | "vsplit"))
            .cloned()
            .collect();
        let settings = settings.clone();
        let (sender, commands) = mpsc::channel::<(usize, String)>();
        let (results, receiver) = mpsc::channel();
        std::thread::Builder::new()
            .name("fvim-picker".into())
            .spawn(move || {
                let files = paths(&root, &settings);
                while let Ok(mut request) = commands.recv() {
                    while let Ok(new) = commands.try_recv() {
                        request = new;
                    }
                    let (entries, mut message) =
                        search(&root, &files, grep, &request.1, settings.picker_max_bytes);
                    if files.len() >= settings.picker_max_files {
                        message.push_str(" (file limit reached)");
                    }
                    if results
                        .send(Results {
                            generation: request.0,
                            entries,
                            message,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        let mut picker = Self {
            grep,
            query: query.into(),
            entries: vec![],
            selected: 0,
            message: "Scanning project…".into(),
            generation: 0,
            sender,
            receiver,
            pending: vec![],
            split_maps,
        };
        picker.refresh();
        Ok(picker)
    }
    fn refresh(&mut self) {
        self.generation += 1;
        self.selected = 0;
        self.entries.clear();
        self.message = "Searching…".into();
        let _ = self.sender.send((self.generation, self.query.clone()));
    }
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(r) = self.receiver.try_recv() {
            if r.generation == self.generation {
                self.entries = r.entries;
                self.message = r.message;
                changed = true;
            }
        }
        changed
    }
    pub fn paste(&mut self, text: &str) {
        self.flush_pending();
        self.query.extend(text.chars().filter(|c| !c.is_control()));
        self.refresh();
    }
    // None: still open; Some(None): cancelled; Some(Some(...)): selected.
    pub fn key(&mut self, key: KeyEvent) -> Option<Option<(Entry, Option<Axis>)>> {
        if key.code == KeyCode::Esc {
            return Some(None);
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && matches!(key.code, KeyCode::Char('v' | 's')) {
            let axis = if key.code == KeyCode::Char('v') {
                Axis::Vertical
            } else {
                Axis::Horizontal
            };
            return self
                .entries
                .get(self.selected)
                .cloned()
                .map(|e| Some((e, Some(axis))));
        }
        if !ctrl && (!self.pending.is_empty() || matches!(key.code, KeyCode::Char(_))) {
            self.pending.push(key);
            if let Some(m) = self
                .split_maps
                .iter()
                .find(|m| m.keys.starts_with(&self.pending))
            {
                if m.keys.len() == self.pending.len() {
                    let axis = if m.action == "vsplit" {
                        Axis::Vertical
                    } else {
                        Axis::Horizontal
                    };
                    self.pending.clear();
                    return self
                        .entries
                        .get(self.selected)
                        .cloned()
                        .map(|e| Some((e, Some(axis))));
                }
                return None;
            }
            self.pending.pop();
            let changed = self.has_pending();
            self.flush_pending();
            // Results for a newly completed query must arrive before it can be opened.
            if changed && key.code == KeyCode::Enter {
                return None;
            }
        }
        match key.code {
            KeyCode::Enter => {
                return self
                    .entries
                    .get(self.selected)
                    .cloned()
                    .map(|e| Some((e, None)))
            }
            KeyCode::Up | KeyCode::BackTab => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Tab => {
                self.selected = (self.selected + 1).min(self.entries.len().saturating_sub(1))
            }
            KeyCode::Char('p') if ctrl => self.selected = self.selected.saturating_sub(1),
            KeyCode::Char('n') if ctrl => {
                self.selected = (self.selected + 1).min(self.entries.len().saturating_sub(1))
            }
            KeyCode::Backspace => {
                self.query.pop();
                self.refresh();
            }
            KeyCode::Char(ch) if !ctrl => {
                self.query.push(ch);
                self.refresh();
            }
            _ => {}
        }
        None
    }
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }
    pub fn flush_pending(&mut self) {
        let pending = std::mem::take(&mut self.pending);
        if !pending.is_empty() {
            for k in pending {
                if let KeyCode::Char(ch) = k.code {
                    self.query.push(ch);
                }
            }
            self.refresh();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fuzzy_ranking_and_unicode_grep_locations() {
        assert!(fuzzy("src/hello.rs", "shrs").is_some());
        assert!(fuzzy("src/hello.rs", "zz").is_none());
        let root = std::env::temp_dir().join(format!("fvim-picker-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("wide.hlsl");
        fs::write(&path, "界 hello\nhello\n").unwrap();
        let (entries, _) = search(&root, &[path], true, "hello", 1024);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].col, 2);
        assert_eq!(entries[1].row, 1);
        fs::remove_dir_all(root).unwrap();
    }
}
