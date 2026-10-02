mod tools;
use crate::{
    buffer::Buffer,
    config::Settings,
    editor::{Editor, Mode, PromptCompletionItem, PromptCompletionMenu},
};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
use std::collections::{HashMap, HashSet};
use std::{path::{Path, PathBuf}, time::Instant};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Axis {
    Vertical,
    Horizontal,
}
#[derive(Clone, Copy, Debug)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

enum Layout {
    Leaf(usize),
    Split(Axis, Box<Layout>, Box<Layout>),
}
impl Layout {
    fn split(&mut self, target: usize, new: usize, axis: Axis) {
        match self {
            Self::Leaf(id) if *id == target => {
                *self = Self::Split(
                    axis,
                    Box::new(Self::Leaf(target)),
                    Box::new(Self::Leaf(new)),
                )
            }
            Self::Split(_, a, b) => {
                a.split(target, new, axis);
                b.split(target, new, axis);
            }
            _ => {}
        }
    }
    fn remove(self, target: usize) -> Option<Self> {
        match self {
            Self::Leaf(id) => (id != target).then_some(Self::Leaf(id)),
            Self::Split(axis, a, b) => match (a.remove(target), b.remove(target)) {
                (Some(a), Some(b)) => Some(Self::Split(axis, Box::new(a), Box::new(b))),
                (a, b) => a.or(b),
            },
        }
    }
    fn rects(&self, r: Rect, out: &mut Vec<(usize, Rect)>) {
        match self {
            Self::Leaf(id) => {
                if r.width > 0 && r.height > 0 {
                    out.push((*id, r));
                }
            }
            Self::Split(axis, a, b) => {
                let mut first = r;
                let mut second = r;
                match axis {
                    Axis::Vertical => {
                        first.width = r.width / 2;
                        second.x += first.width;
                        second.width -= first.width;
                    }
                    Axis::Horizontal => {
                        first.height = r.height / 2;
                        second.y += first.height;
                        second.height -= first.height;
                    }
                }
                a.rects(first, out);
                b.rects(second, out);
            }
        }
    }
}

pub struct Window {
    pub buffer: usize,
    pub row: usize,
    pub col: usize,
    pub goal: Option<usize>,
    pub renderer: crate::renderer::Renderer,
}
fn mapping_key(mut key: KeyEvent) -> KeyEvent {
    key.kind = KeyEventKind::Press;
    key.state = KeyEventState::NONE;
    if matches!(key.code, KeyCode::Char(_)) {
        key.modifiers.remove(KeyModifiers::SHIFT);
    }
    key
}

pub struct Workspace {
    pub buffers: Vec<Editor>,
    pub windows: Vec<Option<Window>>,
    pub active: usize,
    layout: Layout,
    pub root: PathBuf,
    pub config_dir: PathBuf,
    pub settings: Settings,
    pub build: Option<crate::diagnostics::Build>,
    tools: crate::tooling::Tools,
    saved_generations: HashMap<usize, u64>,
    terminal_prefix: bool,
    terminal_window: Option<usize>,
    diagnostics: HashMap<(PathBuf, String), Vec<crate::diagnostics::Diagnostic>>,
    synced: HashMap<usize, (u64, PathBuf, u64, bool)>,
    versions: HashMap<PathBuf, u64>,
    tool_version: u64,
    last_input: Instant,
    statuses: Vec<String>,
    pub picker: Option<crate::picker::Picker>,
    pending_keys: Vec<KeyEvent>,
    pending_at: Instant,
    window_prefix: bool,
    discard_armed: bool,
    completion_allowed: Option<(usize, u64, usize, usize)>,
}

impl Workspace {
    pub fn new(buffer: Buffer, config_dir: PathBuf) -> Self {
        let mut editor = Editor::new(buffer);
        editor.workspace_managed = true;
        let settings = editor.settings.clone();
        let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self {
            buffers: vec![editor],
            windows: vec![Some(Window {
                buffer: 0,
                row: 0,
                col: 0,
                goal: None,
                renderer: Default::default(),
            })],
            active: 0,
            layout: Layout::Leaf(0),
            root,
            config_dir,
            tools: crate::tooling::Tools::new(settings.tooling.clone()),
            build: None,
            saved_generations: HashMap::new(),
            terminal_prefix: false,
            terminal_window: None,
            diagnostics: HashMap::new(),
            synced: HashMap::new(),
            versions: HashMap::new(),
            tool_version: 0,
            last_input: Instant::now(),
            statuses: vec![],
            settings,
            picker: None,
            pending_keys: vec![],
            pending_at: Instant::now(),
            window_prefix: false,
            discard_armed: false,
            completion_allowed: None,
        }
    }
    pub fn editor(&self) -> &Editor {
        &self.buffers[self.windows[self.active].as_ref().unwrap().buffer]
    }
    pub fn editor_mut(&mut self) -> &mut Editor {
        let id = self.windows[self.active].as_ref().unwrap().buffer;
        &mut self.buffers[id]
    }
    pub fn apply_settings(&mut self, settings: Settings) {
        self.tools.settings(settings.tooling.clone());
        self.synced.clear();
        self.saved_generations.clear();
        self.versions.clear();
        self.diagnostics.clear();
        self.statuses.clear();
        self.completion_allowed = None;
        self.settings = settings;
        for editor in &mut self.buffers {
            editor.settings = self.settings.for_path(editor.buffer.path.as_deref());
            editor.diagnostics.clear();
            editor.syntax.clear();
            editor.semantic.clear();
            editor.popup = None;
            editor.completion = None;
            editor.prompt_completion = None;
        }
    }
    fn store_cursor(&mut self) {
        let (row, col) = (self.editor().buffer.row, self.editor().buffer.col);
        let w = self.windows[self.active].as_mut().unwrap();
        w.row = row;
        w.col = col;
        w.goal = self.buffers[w.buffer].goal;
    }
    pub fn focus(&mut self, id: usize) {
        self.store_cursor();
        self.editor_mut().cancel_selection();
        self.active = id;
        let w = self.windows[id].as_ref().unwrap();
        let (row, col, goal) = (w.row, w.col, w.goal);
        let e = self.editor_mut();
        e.goal = goal;
        e.buffer.row = row.min(e.buffer.lines.len() - 1);
        e.buffer.col = col.min(
            e.buffer.lines[e.buffer.row]
                .chars()
                .count()
                .saturating_sub(1),
        );
    }
    fn direction(&mut self, direction: char) {
        let rects = self.rectangles(1000, 1000);
        let Some((_, current)) = rects.iter().find(|(id, _)| *id == self.active) else {
            return;
        };
        let cx = current.x as i32 + current.width as i32 / 2;
        let cy = current.y as i32 + current.height as i32 / 2;
        let candidate = rects
            .iter()
            .filter(|(id, _)| *id != self.active)
            .filter_map(|(id, r)| {
                let dx = r.x as i32 + r.width as i32 / 2 - cx;
                let dy = r.y as i32 + r.height as i32 / 2 - cy;
                let (forward, side) = match direction {
                    'h' => (-dx, dy.abs()),
                    'l' => (dx, dy.abs()),
                    'k' => (-dy, dx.abs()),
                    _ => (dy, dx.abs()),
                };
                (forward > 0).then_some((forward + side * 3, *id))
            })
            .min();
        if let Some((_, id)) = candidate {
            self.focus(id);
        }
    }
    pub fn rectangles(&self, width: u16, height: u16) -> Vec<(usize, Rect)> {
        let mut out = vec![];
        self.layout.rects(
            Rect {
                x: 0,
                y: 0,
                width,
                height,
            },
            &mut out,
        );
        out
    }
    fn open(&mut self, path: Option<PathBuf>) -> Result<(), String> {
        let path = path.map(|p| std::fs::canonicalize(&p).unwrap_or(p));
        let id = path.as_ref().and_then(|p| {
            self.buffers.iter().position(|e| {
                e.buffer.path.as_ref().is_some_and(|stored| {
                    std::fs::canonicalize(stored).unwrap_or_else(|_| stored.clone()) == *p
                })
            })
        });
        let id = if let Some(id) = id {
            id
        } else {
            let mut e = Editor::new(Buffer::open(path).map_err(|e| e.to_string())?);
            e.workspace_managed = true;
            e.settings = self.settings.for_path(e.buffer.path.as_deref());
            self.buffers.push(e);
            self.buffers.len() - 1
        };
        self.store_cursor();
        self.editor_mut().cancel_selection();
        let w = self.windows[self.active].as_mut().unwrap();
        w.buffer = id;
        w.row = self.buffers[id].buffer.row;
        w.col = self.buffers[id].buffer.col;
        w.renderer.invalidate();
        Ok(())
    }
    fn split(&mut self, axis: Axis, path: Option<PathBuf>) -> Result<(), String> {
        // Validate before changing layout so a failed open leaves no extra window.
        if let Some(p) = &path {
            Buffer::open(Some(p.clone())).map_err(|e| e.to_string())?;
        }
        self.store_cursor();
        let old = self.windows[self.active].as_ref().unwrap();
        let new = self.windows.len();
        self.windows.push(Some(Window {
            buffer: old.buffer,
            row: old.row,
            col: old.col,
            goal: old.goal,
            renderer: Default::default(),
        }));
        self.layout.split(self.active, new, axis);
        self.focus(new);
        if path.is_some() {
            self.open(path)?;
        }
        Ok(())
    }
    pub fn command(&mut self, command: &str) -> Result<bool, String> {
        let (name, arg) = command
            .trim()
            .split_once(char::is_whitespace)
            .unwrap_or((command.trim(), ""));
        let arg = arg.trim();
        let force = name.ends_with('!');
        let name = name.trim_end_matches('!');
        match name {
            "q" | "quit" => {
                let count = self.windows.iter().flatten().count();
                let dirty = if count == 1 {
                    self.buffers.iter().any(|e| e.buffer.dirty())
                } else {
                    self.editor().buffer.dirty()
                };
                if dirty && !force {
                    return Err("Unsaved changes. Use :q! to discard them.".into());
                }
                if count == 1 {
                    return Ok(true);
                }
                let old = std::mem::replace(&mut self.layout, Layout::Leaf(self.active));
                self.layout = old.remove(self.active).unwrap();
                self.windows[self.active] = None;
                self.active = self.windows.iter().position(Option::is_some).unwrap();
                let w = self.windows[self.active].as_ref().unwrap();
                let (row, col) = (w.row, w.col);
                let e = self.editor_mut();
                e.buffer.row = row.min(e.buffer.lines.len() - 1);
                e.buffer.col = col.min(
                    e.buffer.lines[e.buffer.row]
                        .chars()
                        .count()
                        .saturating_sub(1),
                );
            }
            "vsplit" | "vs" | "split" | "sp" => self.split(
                if matches!(name, "vsplit" | "vs") {
                    Axis::Vertical
                } else {
                    Axis::Horizontal
                },
                (!arg.is_empty()).then(|| PathBuf::from(arg)),
            )?,
            "e" | "edit" | "enew" => {
                if force {
                    let path = self.editor().buffer.path.clone();
                    self.editor_mut().buffer = Buffer::open(path).map_err(|e| e.to_string())?;
                }
                self.open(if name == "enew" {
                    None
                } else if arg.is_empty() {
                    self.editor().buffer.path.clone()
                } else {
                    Some(PathBuf::from(arg))
                })?;
            }
            "bnext" | "bn" | "bprevious" | "bp" | "buffer" | "b" => {
                self.store_cursor();
                let current = self.windows[self.active].as_ref().unwrap().buffer;
                let id = if matches!(name, "buffer" | "b") {
                    arg.parse::<usize>()
                        .ok()
                        .filter(|n| *n > 0 && *n <= self.buffers.len())
                        .map(|n| n - 1)
                        .ok_or("Unknown buffer number")?
                } else if matches!(name, "bp" | "bprevious") {
                    (current + self.buffers.len() - 1) % self.buffers.len()
                } else {
                    (current + 1) % self.buffers.len()
                };
                self.editor_mut().cancel_selection();
                self.windows[self.active].as_mut().unwrap().buffer = id;
                let (row, col) = (self.buffers[id].buffer.row, self.buffers[id].buffer.col);
                let w = self.windows[self.active].as_mut().unwrap();
                w.row = row;
                w.col = col;
                w.renderer.invalidate();
            }
            "buffers" | "ls" => {
                self.editor_mut().message = self
                    .buffers
                    .iter()
                    .enumerate()
                    .map(|(id, e)| {
                        format!(
                            "{} {}{}",
                            id + 1,
                            e.buffer
                                .path
                                .as_ref()
                                .map_or("[No Name]".into(), |p| p.display().to_string()),
                            if e.buffer.dirty() { " [+]" } else { "" }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" | ")
            }
            "files" | "grep" => {
                self.picker = Some(crate::picker::Picker::new(
                    &self.root,
                    name == "grep",
                    arg,
                    &self.settings,
                )?)
            }
            "config" => self.open(Some(self.config_dir.join("init.lua")))?,
            "filetype" => {
                let settings = self.settings.for_path(self.editor().buffer.path.as_deref());
                self.editor_mut().settings = settings;
            }
            "make" => {
                if self.build.is_some() {
                    return Err("Build already running; Ctrl-C stops it".into());
                }
                self.editor_mut()
                    .buffer
                    .save()
                    .map_err(|e| format!("Save failed: {e}"))?;
                let command = crate::diagnostics::build_command(&self.root, &self.settings, arg)?;
                let id = self.buffers.len();
                let build = crate::diagnostics::Build::start(&self.root, &command, id)?;
                let mut e = Editor::new(Buffer::from_text(&build.text));
                e.workspace_managed = true;
                e.settings = self.settings.clone();
                e.title = Some("[Build]".into());
                e.settings.tooling.diagnostics.virtual_text = false;
                e.message = "Build running; Ctrl-C stops it".into();
                if let Some(window) = self
                    .terminal_window
                    .filter(|id| self.windows.get(*id).is_some_and(Option::is_some))
                {
                    self.focus(window);
                } else {
                    self.split(Axis::Horizontal, None)?;
                    self.terminal_window = Some(self.active);
                }
                e.terminal = Some(build.screen().clone());
                e.mode = Mode::Insert;
                e.message = "Terminal: Ctrl-\\ Ctrl-N returns to Normal; Ctrl-Q stops build".into();
                self.buffers.push(e);
                let w = self.windows[self.active].as_mut().unwrap();
                w.buffer = id;
                w.row = 0;
                w.col = 0;
                w.goal = None;
                self.build = Some(build);
                self.terminal_prefix = false;
                self.diagnostics
                    .retain(|(_, source), _| source != "compiler");
            }
            "diagnostics" => {
                let entries = self.diagnostic_entries();
                let mut picker =
                    crate::picker::Picker::from_entries("Diagnostics", entries, &self.settings);
                picker.message = self.statuses.join(" | ");
                self.picker = Some(picker);
            }
            "diagnostic_next" | "diagnostic_previous" => {
                let position = (self.editor().buffer.row, self.editor().buffer.col);
                let mut entries = self.editor().diagnostics.clone();
                entries.sort_by_key(|d| (d.row, d.col));
                let selected = if name == "diagnostic_next" {
                    entries
                        .iter()
                        .find(|d| (d.row, d.col) > position)
                        .or(entries.first())
                } else {
                    entries
                        .iter()
                        .rev()
                        .find(|d| (d.row, d.col) < position)
                        .or(entries.last())
                };
                if let Some(d) = selected {
                    let (row, col) = (d.row, d.col);
                    let e = self.editor_mut();
                    e.buffer.row = row.min(e.buffer.lines.len() - 1);
                    e.buffer.col = col.min(
                        e.buffer.lines[e.buffer.row]
                            .chars()
                            .count()
                            .saturating_sub(1),
                    );
                }
            }
            "diagnostic_float" => {
                self.cursor_diagnostics();
            }
            "hover" | "definition" | "references" => {
                let e = self.editor();
                let path = e.buffer.path.clone().ok_or("No file for LSP request")?;
                let method = match name {
                    "hover" => "textDocument/hover",
                    "definition" => "textDocument/definition",
                    _ => "textDocument/references",
                };
                self.tools.request(path, e.buffer.row, e.buffer.col, method);
            }
            "comment" | "blockcomment" => {
                let ft = self.settings.filetype(self.editor().buffer.path.as_deref());
                let syntax = self
                    .settings
                    .comments
                    .get(&ft)
                    .ok_or_else(|| format!("No comments configured for {ft}"))?
                    .clone();
                self.editor_mut().toggle_comment(
                    &syntax.0,
                    (name == "blockcomment").then_some((syntax.1.as_str(), syntax.2.as_str())),
                )?;
            }
            _ => return Err(format!("Unknown workflow command: {name}")),
        }
        Ok(false)
    }
    fn action(&mut self, action: &str) -> bool {
        match self.command(action) {
            Ok(quit) => quit,
            Err(e) => {
                self.editor_mut().message = e;
                false
            }
        }
    }
    pub fn flush_keys(&mut self) -> bool {
        let keys = std::mem::take(&mut self.pending_keys);
        for key in keys {
            if self.editor_mut().key(key) {
                return true;
            }
        }
        self.dispatch()
    }
    pub fn has_pending_input(&self) -> bool {
        !self.pending_keys.is_empty() || self.picker.as_ref().is_some_and(|p| p.has_pending())
    }
    pub fn timeout(&mut self) -> bool {
        if self.pending_at.elapsed().as_millis() >= self.settings.mapping_timeout as u128 {
            if let Some(p) = &mut self.picker {
                p.flush_pending();
            }
        }
        if !self.pending_keys.is_empty()
            && self.pending_at.elapsed().as_millis() >= self.settings.mapping_timeout as u128
        {
            self.flush_keys()
        } else {
            false
        }
    }
    fn dispatch(&mut self) -> bool {
        if let Some(cmd) = self.editor_mut().workflow_command.take() {
            self.action(&cmd)
        } else {
            false
        }
    }
    fn prompt_path_value(&self, path: &Path) -> String {
        let value = if let Ok(relative) = path.strip_prefix(&self.root) {
            relative.to_path_buf()
        } else if let (Ok(root), Ok(path)) = (
            std::fs::canonicalize(&self.root),
            std::fs::canonicalize(path),
        ) {
            path.strip_prefix(&root)
                .map(Path::to_path_buf)
                .unwrap_or(path)
        } else {
            path.to_path_buf()
        };
        value.to_string_lossy().replace('\\', "/")
    }

    fn prompt_path_items(&self, text: &str) -> Vec<PromptCompletionItem> {
        let Some((command, argument)) = text.split_once(char::is_whitespace) else {
            return vec![];
        };
        if !matches!(command, "e" | "edit" | "sp" | "split" | "vs" | "vsplit") {
            return vec![];
        }
        let argument = argument.trim_start();
        if argument.is_empty() {
            return vec![];
        }
        let needle = argument.to_lowercase();
        let mut seen = HashSet::new();
        let mut items = Vec::new();

        let current = self.editor().buffer.path.as_ref().map(|p| self.prompt_path_value(p));
        if let Some(value) = current.as_ref().filter(|value| value.to_lowercase().starts_with(&needle)) {
            seen.insert(value.clone());
            items.push(PromptCompletionItem {
                label: format!("[current] {value}"),
                value: format!("{command} {value}"),
                directory: false,
            });
        }
        for editor in &self.buffers {
            let Some(path) = editor.buffer.path.as_ref() else {
                continue;
            };
            let value = self.prompt_path_value(path);
            if value.to_lowercase().starts_with(&needle) && seen.insert(value.clone()) {
                items.push(PromptCompletionItem {
                    label: format!("[open] {value}"),
                    value: format!("{command} {value}"),
                    directory: false,
                });
            }
        }

        let separator = argument.rfind(|ch| ch == '/' || ch == '\\');
        let (directory_text, fragment) = match separator {
            Some(index) => (&argument[..=index], &argument[index + 1..]),
            None => ("", argument),
        };
        let directory_path = if let Some(rest) = directory_text.strip_prefix("~/") {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| self.root.clone())
                .join(rest)
        } else {
            let path = PathBuf::from(if directory_text.is_empty() { "." } else { directory_text });
            if path.is_absolute() {
                path
            } else {
                self.root.join(path)
            }
        };
        let fragment = fragment.to_lowercase();
        let mut filesystem = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&directory_path) {
            for entry in entries.flatten().take(self.settings.picker_max_files.min(2000)) {
                let name = entry.file_name().to_string_lossy().into_owned();
                if !name.to_lowercase().starts_with(&fragment) {
                    continue;
                }
                let directory = entry.file_type().is_ok_and(|kind| kind.is_dir());
                let mut value = format!("{directory_text}{name}");
                if directory {
                    value.push('/');
                }
                if !seen.insert(value.clone()) {
                    continue;
                }
                filesystem.push(PromptCompletionItem {
                    label: format!("{} {value}", if directory { "[dir]" } else { "[file]" }),
                    value: format!("{command} {value}"),
                    directory,
                });
            }
        }
        filesystem.sort_by(|a, b| {
            b.directory
                .cmp(&a.directory)
                .then_with(|| a.label.to_lowercase().cmp(&b.label.to_lowercase()))
        });
        items.extend(filesystem);
        items.truncate(200);
        items
    }

    fn prompt_search_items(&self, prefix: &str) -> Vec<PromptCompletionItem> {
        if prefix.is_empty() || !prefix.chars().all(|ch| ch.is_alphanumeric() || ch == '_') {
            return vec![];
        }
        let folded = prefix.to_lowercase();
        let mut seen = HashSet::new();
        let mut words = Vec::new();
        for line in &self.editor().buffer.lines {
            for word in line.split(|ch: char| !ch.is_alphanumeric() && ch != '_') {
                if word.len() <= prefix.len()
                    || !word.to_lowercase().starts_with(&folded)
                    || !seen.insert(word.to_owned())
                {
                    continue;
                }
                words.push(word.to_owned());
                if words.len() >= 200 {
                    break;
                }
            }
            if words.len() >= 200 {
                break;
            }
        }
        words.sort_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)));
        words
            .into_iter()
            .map(|word| PromptCompletionItem {
                label: format!("[syntax] {word}"),
                value: word,
                directory: false,
            })
            .collect()
    }

    fn refresh_prompt_completion(&mut self) {
        let Some((kind, text)) = self
            .editor()
            .prompt
            .as_ref()
            .map(|prompt| (prompt.kind, prompt.text.clone()))
        else {
            self.editor_mut().prompt_completion = None;
            return;
        };
        let items = if kind == ':' {
            self.prompt_path_items(&text)
        } else if matches!(kind, '/' | '?') {
            self.prompt_search_items(&text)
        } else {
            vec![]
        };
        self.editor_mut().prompt_completion = (!items.is_empty()).then_some(PromptCompletionMenu {
            items,
            selected: None,
        });
    }

    fn prompt_completion_key(&mut self, key: KeyEvent) -> Option<bool> {
        if !key.modifiers.is_empty() {
            return None;
        }
        match key.code {
            KeyCode::Up | KeyCode::Down => {
                let menu = self.editor_mut().prompt_completion.as_mut()?;
                let reverse = key.code == KeyCode::Up;
                menu.selected = Some(match menu.selected {
                    None => if reverse { menu.items.len() - 1 } else { 0 },
                    Some(index) if reverse => (index + menu.items.len() - 1) % menu.items.len(),
                    Some(index) => (index + 1) % menu.items.len(),
                });
                Some(false)
            }
            KeyCode::Enter => {
                let item = {
                    let menu = self.editor().prompt_completion.as_ref()?;
                    menu.selected.and_then(|index| menu.items.get(index)).cloned()?
                };
                self.editor_mut().prompt.as_mut().unwrap().text = item.value;
                self.editor_mut().prompt_completion = None;
                if item.directory {
                    self.refresh_prompt_completion();
                    Some(false)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    pub fn key(&mut self, key: KeyEvent) -> bool {
        self.last_input = Instant::now();
        self.editor_mut().popup = None;
        if self.terminal_input() {
            let control = key.modifiers.contains(KeyModifiers::CONTROL);
            if self.terminal_prefix {
                self.terminal_prefix = false;
                if control && key.code == KeyCode::Char('n') {
                    self.editor_mut().mode = Mode::Normal;
                    self.editor_mut().message =
                        "Terminal Normal: i resumes input; Ctrl-W changes pane".into();
                    return false;
                }
                if let Some(build) = &mut self.build {
                    let _ = build.input(b"\x1c");
                }
            }
            if control && matches!(key.code, KeyCode::Char('\\' | '4' | '\x1c')) {
                self.terminal_prefix = true;
                return false;
            }
            if let Some(build) = &mut self.build {
                if control && key.code == KeyCode::Char('q') {
                    build.cancel();
                } else {
                    let _ = build.input(&crate::terminal_key(key));
                }
            }
            return false;
        }
        if self.editor().terminal.is_some()
            && self.editor().mode == Mode::Normal
            && self.editor().mapping_ready()
            && !self.window_prefix
            && self.pending_keys.is_empty()
            && key.code == KeyCode::Char('i')
        {
            self.editor_mut().mode = Mode::Insert;
            self.editor_mut().message = "Terminal: Ctrl-\\ Ctrl-N returns to Normal".into();
            return false;
        }
        let system_shortcut = key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER);
        if system_shortcut && self.editor().terminal.is_none() && self.editor().mode != Mode::Insert
        {
            match key.code {
                KeyCode::Char('c') => {
                    let text = self.editor().clipboard_text();
                    if let Err(error) = crate::clipboard::set(&text) {
                        self.editor_mut().message = format!("Clipboard copy failed: {error}");
                    }
                    return false;
                }
                KeyCode::Char('v') if self.editor().mode == Mode::Normal => {
                    match crate::clipboard::get() {
                        Ok(text) if !text.is_empty() => self.editor_mut().paste(&text),
                        Ok(_) => {}
                        Err(error) => {
                            self.editor_mut().message = format!("Clipboard paste failed: {error}")
                        }
                    }
                    return false;
                }
                _ => {}
            }
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            if let Some(build) = &mut self.build {
                build.cancel();
                return false;
            }
        }
        if self.editor().prompt.is_some() {
            if let Some(quit) = self.prompt_completion_key(key) {
                return quit;
            }
            let quit = self.editor_mut().key(key);
            if self.editor().prompt.is_some() {
                self.refresh_prompt_completion();
            } else {
                self.editor_mut().prompt_completion = None;
            }
            return quit || self.dispatch();
        }
        if self.editor().mode == Mode::Insert {
            if key.modifiers.contains(KeyModifiers::CONTROL)
                && matches!(key.code, KeyCode::Null | KeyCode::Char(' '))
            {
                let id = self.windows[self.active].as_ref().unwrap().buffer;
                let (path, revision, row, col) = {
                    let e = self.editor();
                    (
                        e.buffer.path.clone(),
                        e.buffer.revision,
                        e.buffer.row,
                        e.buffer.col,
                    )
                };
                if let Some(path) = path {
                    self.completion_allowed = Some((id, revision, row, col));
                    self.tools.request(path, row, col, "textDocument/completion");
                }
                return false;
            }
            if self.completion_key(key) {
                return false;
            }
        }

        if self.picker.is_some() {
            self.pending_at = Instant::now();
            return self.picker_key(key);
        }
        if self.window_prefix {
            self.window_prefix = false;
            match key.code {
                KeyCode::Char('w') => {
                    let ids: Vec<_> = self
                        .windows
                        .iter()
                        .enumerate()
                        .filter_map(|(i, w)| w.as_ref().map(|_| i))
                        .collect();
                    let next =
                        (ids.iter().position(|i| *i == self.active).unwrap() + 1) % ids.len();
                    self.focus(ids[next]);
                }
                KeyCode::Char(ch @ ('h' | 'j' | 'k' | 'l')) => self.direction(ch),
                KeyCode::Char('v') => {
                    self.action("vsplit");
                }
                KeyCode::Char('s') => {
                    self.action("split");
                }
                KeyCode::Char('q') => return self.action("q"),
                _ => {}
            }
            return false;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('q') {
            if self.buffers.iter().any(|e| e.buffer.dirty()) && !self.discard_armed {
                self.discard_armed = true;
                self.editor_mut().message =
                    "Unsaved changes. Ctrl-Q again discards all buffers.".into();
                return false;
            }
            return true;
        }
        self.discard_armed = false;
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && key.code == KeyCode::Char('w')
            && self.editor().mapping_ready()
        {
            self.flush_keys();
            self.window_prefix = true;
            return false;
        }
        if key.code == KeyCode::Esc && !self.pending_keys.is_empty() {
            self.pending_keys.clear();
            return self.editor_mut().key(key);
        }
        if self.editor().mapping_ready() || !self.pending_keys.is_empty() {
            let key = mapping_key(key);
            self.pending_keys.push(key);
            self.pending_at = Instant::now();
            let mode = if self.editor().mode == Mode::Normal {
                "n"
            } else {
                "v"
            };
            let mut matching = self
                .settings
                .keymaps
                .iter()
                .filter(|m| m.mode == mode && m.keys.starts_with(&self.pending_keys));
            if let Some(m) = matching.next() {
                if m.keys.len() == self.pending_keys.len() {
                    let action = m.action.clone();
                    self.pending_keys.clear();
                    return self.action(&action);
                }
                return false;
            }
            // Replay the unmatched prefix through Vim, then allow the last key to start a new mapping.
            let last = self.pending_keys.pop().unwrap();
            if !self.pending_keys.is_empty() {
                if self.flush_keys() {
                    return true;
                }
                return self.key(last);
            }
        }
        let inserting = self.editor().mode == Mode::Insert;
        let typed_completion_character = inserting
            && key.modifiers.is_empty()
            && matches!(key.code, KeyCode::Char(ch) if !ch.is_whitespace());
        let quit = self.editor_mut().key(key);
        if inserting {
            let id = self.windows[self.active].as_ref().unwrap().buffer;
            if typed_completion_character && self.editor().mode == Mode::Insert {
                let (revision, row, col) = {
                    let e = self.editor();
                    (e.buffer.revision, e.buffer.row, e.buffer.col)
                };
                self.completion_allowed = Some((id, revision, row, col));
            } else {
                self.completion_allowed = None;
            }
        }
        if inserting && self.editor().mode == Mode::Normal {
            self.refresh_diagnostics();
        }
        if self.editor().prompt.is_some() {
            self.refresh_prompt_completion();
        }
        quit || self.dispatch()
    }
    fn picker_key(&mut self, key: KeyEvent) -> bool {
        let selection = self.picker.as_mut().unwrap().key(key);
        if let Some(selection) = selection {
            self.picker = None;
            if let Some((entry, axis)) = selection {
                if entry.path.as_os_str().is_empty() {
                    if let Some(id) = self
                        .buffers
                        .iter()
                        .rposition(|e| e.title.as_deref() == Some("[Build]"))
                    {
                        let _ = self.command(&format!("buffer {}", id + 1));
                    }
                    return false;
                }
                let result = if let Some(axis) = axis {
                    self.split(axis, Some(entry.path.clone()))
                } else {
                    self.open(Some(entry.path.clone()))
                };
                match result {
                    Ok(()) => {
                        self.editor_mut().buffer.row =
                            entry.row.min(self.editor().buffer.lines.len() - 1);
                        self.editor_mut().buffer.col = entry.col;
                    }
                    Err(e) => self.editor_mut().message = e,
                }
            }
        }
        false
    }
    pub fn terminal_write(&mut self, bytes: &[u8]) {
        self.last_input = Instant::now();
        if let Some(build) = &mut self.build {
            let _ = build.input(bytes);
        }
    }

    pub fn terminal_input(&self) -> bool {
        self.editor().terminal.is_some()
            && self.editor().mode == Mode::Insert
            && self.build.as_ref().is_some_and(|build| {
                self.windows[self.active].as_ref().unwrap().buffer == build.buffer
            })
    }
    pub fn resize_terminal(&mut self, size: (u16, u16)) {
        let Some(id) = self.build.as_ref().map(|build| build.buffer) else {
            return;
        };
        let rects = self.rectangles(
            size.0,
            size.1.saturating_sub(self.settings.cmdheight.max(1) as u16),
        );
        let Some((_, rect)) = rects
            .into_iter()
            .find(|(window, _)| self.windows[*window].as_ref().unwrap().buffer == id)
        else {
            return;
        };
        let cols = rect
            .width
            .saturating_sub(u16::from(rect.x + rect.width < size.0))
            .max(1);
        let rows = rect
            .height
            .saturating_sub(u16::from(self.settings.laststatus >= 2))
            .max(1);
        let build = self.build.as_mut().unwrap();
        if build.screen().size() != (rows, cols) {
            build.resize(cols, rows);
            self.buffers[id].terminal = Some(build.screen().clone());
        }
    }
    pub fn paste(&mut self, text: &str) {
        if self.terminal_input() {
            if let Some(build) = &mut self.build {
                let _ = build.input(text.as_bytes());
            }
            return;
        }
        self.last_input = Instant::now();
        self.editor_mut().completion = None;
        self.editor_mut().popup = None;
        if let Some(p) = &mut self.picker {
            p.paste(text);
        } else {
            self.flush_keys();
            self.editor_mut().paste(text);
            if self.editor().prompt.is_some() {
                self.refresh_prompt_completion();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(w: &mut Workspace, ch: char) {
        w.key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }

    #[test]
    fn automatic_completion_is_armed_only_by_typed_nonwhitespace_characters() {
        let mut w = Workspace::new(Buffer::from_text(""), PathBuf::from("."));
        key(&mut w, 'i');
        assert!(w.completion_allowed.is_none());

        key(&mut w, 'x');
        assert!(w.completion_allowed.is_some());

        w.key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert!(w.completion_allowed.is_none());

        key(&mut w, '.');
        assert!(w.completion_allowed.is_some());

        key(&mut w, ' ');
        assert!(w.completion_allowed.is_none());
    }

    #[test]
    fn edit_prompt_completes_current_open_files_and_directories() {
        let root = std::env::temp_dir().join(format!("fvim-prompt-{}", std::process::id()));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.c"), "int main;").unwrap();
        std::fs::write(root.join("second.c"), "int second;").unwrap();

        let mut first = Buffer::open(Some(root.join("second.c"))).unwrap();
        first.row = 0;
        let mut w = Workspace::new(first, PathBuf::from("."));
        w.root = root.clone();

        for ch in ":e s".chars() {
            key(&mut w, ch);
        }
        let menu = w.editor().prompt_completion.as_ref().unwrap();
        assert!(menu.items.iter().any(|item| item.label.contains("[current] second.c")));
        assert!(menu.items.iter().any(|item| item.label.contains("[dir] src/")));

        key(&mut w, 'r');
        key(&mut w, 'c');
        key(&mut w, '/');
        key(&mut w, 'm');
        let menu = w.editor().prompt_completion.as_ref().unwrap();
        assert!(menu.items.iter().any(|item| item.value == "e src/main.c"));

        w.key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        w.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(w
            .editor()
            .buffer
            .path
            .as_ref()
            .is_some_and(|path| path.ends_with("src/main.c")));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn search_prompt_completes_identifiers_from_the_focused_file() {
        let mut w = Workspace::new(
            Buffer::from_text("alphaThing beta\nalphaOther gamma"),
            PathBuf::from("."),
        );
        for ch in "/alp".chars() {
            key(&mut w, ch);
        }
        let menu = w.editor().prompt_completion.as_ref().unwrap();
        assert!(menu.items.iter().any(|item| item.value == "alphaThing"));
        assert!(menu.items.iter().any(|item| item.value == "alphaOther"));

        w.key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        w.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(w.editor().prompt.is_none());
    }

    #[test]
    fn split_vertical_motion_goals_are_independent() {
        let mut w = Workspace::new(
            Buffer::from_text("long line\nsecond long line"),
            PathBuf::from("."),
        );
        w.command("vsplit").unwrap();
        key(&mut w, '$');
        w.focus(0);
        key(&mut w, 'j');
        assert_eq!(w.editor().buffer.col, 0);
        w.focus(1);
        key(&mut w, 'j');
        assert_eq!(w.editor().buffer.col, 15);
    }

    #[test]
    fn saving_an_unnamed_file_applies_filetype_indentation() {
        let path = std::env::temp_dir().join(format!("fvim-indent-{}.llvm", std::process::id()));
        let mut w = Workspace::new(Buffer::from_text(""), PathBuf::from("."));
        for ch in format!(":w {}", path.display()).chars() {
            key(&mut w, ch);
        }
        w.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(w.editor().settings.shiftwidth, 2);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn new_relative_files_reopen_as_the_same_shared_buffer() {
        let path = PathBuf::from(format!("fvim-relative-{}.tmp", std::process::id()));
        let mut w = Workspace::new(
            Buffer::open(Some(path.clone())).unwrap(),
            PathBuf::from("."),
        );
        key(&mut w, 'i');
        key(&mut w, 'X');
        w.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        w.editor_mut().buffer.save().unwrap();
        w.command(&format!("vsplit {}", path.display())).unwrap();
        assert_eq!(w.buffers.len(), 1);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn forced_edit_reloads_clean_files_changed_outside_the_editor() {
        let path = std::env::temp_dir().join(format!("fvim-reload-{}", std::process::id()));
        std::fs::write(&path, "before").unwrap();
        let mut w = Workspace::new(
            Buffer::open(Some(path.clone())).unwrap(),
            PathBuf::from("."),
        );
        std::fs::write(&path, "after").unwrap();
        w.command("e!").unwrap();
        assert_eq!(w.editor().buffer.text(), "after");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn make_saves_before_running_and_keeps_source_buffer_available() {
        let path = std::env::temp_dir().join(format!("fvim-make-{}", std::process::id()));
        let mut b = Buffer::from_text("source");
        b.path = Some(path.clone());
        let mut w = Workspace::new(b, PathBuf::from("."));
        w.settings.make_command = vec![
            std::env::current_exe()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            "--list".into(),
        ];
        key(&mut w, 'i');
        key(&mut w, 'X');
        w.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        w.command("make").unwrap();
        let deadline = Instant::now() + std::time::Duration::from_secs(10);
        while w.build.is_some() && Instant::now() < deadline {
            w.poll();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(w.build.is_none());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "Xsource");
        assert!(w.editor().buffer.body().contains("test"));
        w.command("bprevious").unwrap();
        assert_eq!(w.editor().buffer.text(), "Xsource");
        assert!(!w.editor().buffer.dirty());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn failed_split_leaves_layout_and_active_buffer_unchanged() {
        let mut w = Workspace::new(Buffer::from_text("source"), PathBuf::from("."));
        let root = std::env::temp_dir();
        assert!(w.command(&format!("vsplit {}", root.display())).is_err());
        assert_eq!(w.windows.len(), 1);
        assert_eq!(w.editor().buffer.text(), "source");
    }

    #[test]
    fn write_quit_protects_dirty_hidden_buffers() {
        let mut w = Workspace::new(Buffer::from_text("old"), PathBuf::from("."));
        key(&mut w, 'i');
        key(&mut w, 'X');
        w.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        let path = std::env::temp_dir().join(format!("fvim-wq-{}", std::process::id()));
        w.command("enew").unwrap();
        w.editor_mut().buffer.path = Some(path.clone());
        for ch in ":wq".chars() {
            key(&mut w, ch);
        }
        assert!(!w.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
        assert!(w.editor().message.contains("Unsaved"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn mapped_prefixes_replay_vim_motions_and_operators() {
        let mut w = Workspace::new(Buffer::from_text("abc\ndef"), PathBuf::from("."));
        key(&mut w, 'l');
        key(&mut w, 'h');
        w.flush_keys();
        assert_eq!(w.editor().buffer.col, 0);
        key(&mut w, 'j');
        key(&mut w, 'g');
        key(&mut w, 'g');
        assert_eq!(w.editor().buffer.row, 0);
        key(&mut w, 'd');
        key(&mut w, 'h');
        assert_eq!(w.editor().buffer.lines[0], "abc");
        key(&mut w, 'g');
        key(&mut w, 'c');
        key(&mut w, 'c');
        assert!(w.editor().message.contains("No comments"));
        assert!(w.pending_keys.is_empty());
    }

    #[test]
    fn comments_follow_filetype_and_are_one_undo_transaction() {
        let mut w = Workspace::new(Buffer::from_text("  alpha\n  beta"), PathBuf::from("."));
        w.editor_mut().buffer.path = Some(PathBuf::from("test.hlsl"));
        key(&mut w, 'V');
        key(&mut w, 'j');
        w.command("comment").unwrap();
        assert_eq!(w.editor().buffer.body(), "  // alpha\n  // beta");
        key(&mut w, 'u');
        assert_eq!(w.editor().buffer.body(), "  alpha\n  beta");
        w.command("blockcomment").unwrap();
        w.command("blockcomment").unwrap();
        assert_eq!(w.editor().buffer.lines[0], "  alpha");
    }

    #[test]
    fn legacy_installed_config_still_runs_visual_gcc_without_dropping_selection() {
        let settings = crate::config::Config::from_scripts(
            "fvim.keymaps={}; fvim.comments={}; vim.opt.number=true",
            "",
        )
        .unwrap()
        .settings;
        let mut w = Workspace::new(Buffer::from_text("alpha\nbeta\ngamma"), PathBuf::from("."));
        w.editor_mut().buffer.path = Some(PathBuf::from("legacy.c"));
        w.apply_settings(settings);

        key(&mut w, 'V');
        key(&mut w, 'j');
        key(&mut w, 'g');
        assert!(matches!(w.editor().mode, Mode::Visual(_)));
        key(&mut w, 'c');
        assert!(matches!(w.editor().mode, Mode::Visual(_)));
        key(&mut w, 'c');

        assert_eq!(w.editor().buffer.body(), "// alpha\n// beta\ngamma");
        assert_eq!(w.editor().mode, Mode::Normal);
    }

    #[test]
    fn visual_mapping_ignores_terminal_key_metadata() {
        let mut w = Workspace::new(Buffer::from_text("alpha\nbeta\ngamma"), PathBuf::from("."));
        w.editor_mut().buffer.path = Some(PathBuf::from("test.c"));
        key(&mut w, 'V');
        key(&mut w, 'j');

        for ch in ['g', 'c', 'c'] {
            w.key(KeyEvent {
                code: KeyCode::Char(ch),
                modifiers: KeyModifiers::NONE,
                kind: KeyEventKind::Repeat,
                state: KeyEventState::CAPS_LOCK,
            });
        }

        assert_eq!(w.editor().buffer.body(), "// alpha\n// beta\ngamma");
        assert_eq!(w.editor().mode, Mode::Normal);
    }

    #[test]
    fn visual_gcc_and_gbc_toggle_multiple_lines_through_the_real_keymaps() {
        let mut w = Workspace::new(Buffer::from_text("alpha\nbeta\ngamma"), PathBuf::from("."));
        w.editor_mut().buffer.path = Some(PathBuf::from("test.c"));

        key(&mut w, 'V');
        key(&mut w, 'j');
        key(&mut w, 'g');
        key(&mut w, 'c');
        key(&mut w, 'c');
        assert_eq!(w.editor().buffer.body(), "// alpha\n// beta\ngamma");
        key(&mut w, 'u');
        assert_eq!(w.editor().buffer.body(), "alpha\nbeta\ngamma");

        w.editor_mut().buffer.row = 0;
        w.editor_mut().buffer.col = 0;
        key(&mut w, 'V');
        key(&mut w, 'j');
        key(&mut w, 'g');
        key(&mut w, 'b');
        key(&mut w, 'c');
        assert_eq!(w.editor().buffer.body(), "/* alpha\nbeta */\ngamma");
        key(&mut w, 'u');
        assert_eq!(w.editor().buffer.body(), "alpha\nbeta\ngamma");
    }

    #[test]
    fn split_shares_edits_but_keeps_independent_cursors_and_undo() {
        let mut w = Workspace::new(Buffer::from_text("alpha\nbeta"), PathBuf::from("."));
        w.command("vsplit").unwrap();
        assert_eq!(w.windows.len(), 2);
        key(&mut w, 'j');
        key(&mut w, 'i');
        key(&mut w, 'X');
        w.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        w.focus(0);
        assert_eq!(w.editor().buffer.row, 0);
        assert_eq!(w.editor().buffer.lines[1], "Xbeta");
        key(&mut w, 'u');
        assert_eq!(w.editor().buffer.lines[1], "beta");
    }

    #[test]
    fn hidden_dirty_buffers_are_retained_and_protect_quit() {
        let mut w = Workspace::new(Buffer::from_text("old"), PathBuf::from("."));
        key(&mut w, 'i');
        key(&mut w, 'X');
        w.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        w.command("enew").unwrap();
        assert_eq!(w.buffers.len(), 2);
        assert!(w.command("q").is_err());
        w.command("bprevious").unwrap();
        assert_eq!(w.editor().buffer.lines[0], "Xold");
        assert!(w.editor().buffer.dirty());
    }

    #[test]
    fn nested_split_rectangles_fit_tiny_and_normal_terminals() {
        let mut w = Workspace::new(Buffer::from_text("x"), PathBuf::from("."));
        w.command("vsplit").unwrap();
        w.command("split").unwrap();
        for width in 1..30 {
            for height in 1..15 {
                for (_, r) in w.rectangles(width, height) {
                    assert!(r.x + r.width <= width && r.y + r.height <= height);
                }
            }
        }
        assert_eq!(w.rectangles(80, 24).len(), 3);
    }
}
