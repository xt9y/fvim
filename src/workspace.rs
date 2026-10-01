use crate::{
    buffer::Buffer,
    config::Settings,
    editor::{Editor, Mode},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::{path::PathBuf, time::Instant};

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
pub struct Workspace {
    pub buffers: Vec<Editor>,
    pub windows: Vec<Option<Window>>,
    pub active: usize,
    layout: Layout,
    pub root: PathBuf,
    pub config_dir: PathBuf,
    pub settings: Settings,
    pub picker: Option<crate::picker::Picker>,
    pending_keys: Vec<KeyEvent>,
    pending_at: Instant,
    window_prefix: bool,
    discard_armed: bool,
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
            settings,
            picker: None,
            pending_keys: vec![],
            pending_at: Instant::now(),
            window_prefix: false,
            discard_armed: false,
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
        self.settings = settings;
        for editor in &mut self.buffers {
            editor.settings = self.settings.for_path(editor.buffer.path.as_deref());
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
                self.editor_mut()
                    .buffer
                    .save()
                    .map_err(|e| format!("Save failed: {e}"))?;
                let mut parts = self.settings.make_command.iter();
                let mut c = std::process::Command::new(parts.next().ok_or("Empty make command")?);
                let result = c
                    .args(parts)
                    .current_dir(&self.root)
                    .stdin(std::process::Stdio::null())
                    .output()
                    .map_err(|e| format!("Build failed: {e}"))?;
                let text = format!(
                    "{}{}",
                    String::from_utf8_lossy(&result.stdout),
                    String::from_utf8_lossy(&result.stderr)
                );
                let mut e = Editor::new(Buffer::from_text(&text));
                e.workspace_managed = true;
                e.settings = self.settings.clone();
                e.message = format!("Build {}", result.status);
                self.buffers.push(e);
                self.windows[self.active].as_mut().unwrap().buffer = self.buffers.len() - 1;
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
    pub fn key(&mut self, key: KeyEvent) -> bool {
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
        let quit = self.editor_mut().key(key);
        quit || self.dispatch()
    }
    fn picker_key(&mut self, key: KeyEvent) -> bool {
        let selection = self.picker.as_mut().unwrap().key(key);
        if let Some(selection) = selection {
            self.picker = None;
            if let Some((entry, axis)) = selection {
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
    pub fn paste(&mut self, text: &str) {
        if let Some(p) = &mut self.picker {
            p.paste(text);
        } else {
            self.flush_keys();
            self.editor_mut().paste(text);
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
