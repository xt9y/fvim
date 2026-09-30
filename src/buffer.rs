use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
struct Snapshot {
    lines: Vec<String>,
    row: usize,
    col: usize,
}

pub struct Buffer {
    pub lines: Vec<String>,
    pub row: usize,
    pub col: usize,
    pub path: Option<PathBuf>,
    crlf: bool,
    saved: String,
    undo: Vec<Snapshot>,
}

impl Buffer {
    pub fn open(path: Option<PathBuf>) -> io::Result<Self> {
        let (text, path) = match path {
            Some(path) => match fs::read_to_string(&path) {
                Ok(text) => (text, Some(fs::canonicalize(path)?)),
                Err(e) if e.kind() == io::ErrorKind::NotFound => (String::new(), Some(path)),
                Err(e) => return Err(e),
            },
            None => (String::new(), None),
        };
        let mut buffer = Self::from_text(&text);
        buffer.path = path;
        Ok(buffer)
    }

    fn from_text(text: &str) -> Self {
        let crlf = text.contains("\r\n");
        let normalized = text.replace("\r\n", "\n");
        Self {
            lines: normalized.split('\n').map(str::to_owned).collect(),
            row: 0,
            col: 0,
            path: None,
            crlf,
            saved: text.to_owned(),
            undo: Vec::new(),
        }
    }

    pub fn text(&self) -> String {
        self.lines.join(if self.crlf { "\r\n" } else { "\n" })
    }

    pub fn dirty(&self) -> bool {
        self.text() != self.saved
    }

    fn checkpoint(&mut self) {
        self.undo.push(Snapshot {
            lines: self.lines.clone(),
            row: self.row,
            col: self.col,
        });
    }

    fn byte_col(&self) -> usize {
        self.lines[self.row]
            .char_indices()
            .nth(self.col)
            .map_or(self.lines[self.row].len(), |(byte, _)| byte)
    }

    pub fn insert(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.checkpoint();
        let text = text.replace("\r\n", "\n");
        let byte = self.byte_col();
        let tail = self.lines[self.row].split_off(byte);
        let mut parts = text.split('\n');
        let first = parts.next().unwrap();
        self.lines[self.row].push_str(first);
        self.col += first.chars().count();
        for part in parts {
            self.row += 1;
            self.lines.insert(self.row, part.to_owned());
            self.col = part.chars().count();
        }
        self.lines[self.row].push_str(&tail);
    }

    pub fn backspace(&mut self) {
        if self.col > 0 {
            self.checkpoint();
            self.col -= 1;
            let byte = self.byte_col();
            self.lines[self.row].remove(byte);
        } else if self.row > 0 {
            self.checkpoint();
            let line = self.lines.remove(self.row);
            self.row -= 1;
            self.col = self.lines[self.row].chars().count();
            self.lines[self.row].push_str(&line);
        }
    }

    pub fn delete(&mut self) {
        if self.col < self.lines[self.row].chars().count() {
            self.checkpoint();
            let byte = self.byte_col();
            self.lines[self.row].remove(byte);
        } else if self.row + 1 < self.lines.len() {
            self.checkpoint();
            let line = self.lines.remove(self.row + 1);
            self.lines[self.row].push_str(&line);
        }
    }

    pub fn undo(&mut self) {
        if let Some(snapshot) = self.undo.pop() {
            self.lines = snapshot.lines;
            self.row = snapshot.row;
            self.col = snapshot.col;
        }
    }

    pub fn move_cursor(&mut self, dx: isize, dy: isize) {
        self.row = self.row.saturating_add_signed(dy).min(self.lines.len() - 1);
        self.col = self
            .col
            .saturating_add_signed(dx)
            .min(self.lines[self.row].chars().count());
    }

    pub fn save(&mut self) -> io::Result<()> {
        let path = self.path.as_ref().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "Open a filename before saving.")
        })?;
        let text = self.text();
        write_atomic(path, text.as_bytes())?;
        self.saved = text;
        Ok(())
    }
}

pub fn write_atomic(path: &Path, content: &[u8]) -> io::Result<()> {
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = path.file_name().ok_or_else(|| io::Error::other("Missing filename."))?;
    for attempt in 0..100 {
        let temporary = parent.join(format!(
            ".{}.fvim-{}-{attempt}",
            name.to_string_lossy(),
            std::process::id()
        ));
        let mut file = match OpenOptions::new().write(true).create_new(true).open(&temporary) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        };
        let result = (|| {
            file.write_all(content)?;
            file.sync_all()?;
            if let Ok(metadata) = fs::metadata(path) {
                file.set_permissions(metadata.permissions())?;
            }
            drop(file);
            fs::rename(&temporary, path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        return result;
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, "Temporary file collision."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_editing_and_line_merges() {
        let mut b = Buffer::from_text("aé界\nlast\n");
        b.move_cursor(2, 0);
        b.insert("🙂\nx");
        assert_eq!(b.text(), "aé🙂\nx界\nlast\n");
        b.backspace();
        b.backspace();
        assert_eq!(b.text(), "aé🙂界\nlast\n");
        b.delete();
        assert_eq!(b.text(), "aé🙂\nlast\n");
        b.undo();
        assert_eq!(b.text(), "aé🙂界\nlast\n");
    }

    #[test]
    fn preserves_crlf_and_trailing_newline() {
        let mut b = Buffer::from_text("one\r\ntwo\r\n");
        assert!(!b.dirty());
        b.insert("X");
        assert_eq!(b.text(), "Xone\r\ntwo\r\n");
        b.undo();
        assert!(!b.dirty());
    }

    #[test]
    fn cursor_and_empty_buffer_are_bounded() {
        let mut b = Buffer::from_text("");
        b.move_cursor(-100, -100);
        b.backspace();
        b.delete();
        b.move_cursor(100, 100);
        assert_eq!((b.row, b.col), (0, 0));
        assert_eq!(b.text(), "");
    }

    #[test]
    fn save_replaces_existing_file_and_preserves_contents() {
        let root = std::env::temp_dir().join(format!("fvim-save-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("unicode file.txt");
        fs::write(&path, "original\r\n").unwrap();
        let mut b = Buffer::open(Some(path.clone())).unwrap();
        b.insert("é界");
        b.save().unwrap();
        assert!(!b.dirty());
        assert_eq!(fs::read_to_string(&path).unwrap(), "é界original\r\n");
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_save_keeps_changes() {
        let mut b = Buffer::from_text("");
        b.insert("unsaved");
        assert!(b.save().is_err());
        assert!(b.dirty());
    }
}
