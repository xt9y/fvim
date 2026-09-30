use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
struct Snapshot {
    lines: Vec<String>,
    row: usize,
    col: usize,
    eol: bool,
}

pub struct Buffer {
    pub lines: Vec<String>,
    pub row: usize,
    pub col: usize,
    pub path: Option<PathBuf>,
    crlf: bool,
    eol: bool,
    saved: String,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    change: Option<Snapshot>,
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

    pub fn from_text(text: &str) -> Self {
        let crlf = text.contains("\r\n");
        let normalized = text.replace("\r\n", "\n");
        let eol = normalized.ends_with('\n');
        let body = if eol {
            &normalized[..normalized.len() - 1]
        } else {
            &normalized
        };
        Self {
            lines: body.split('\n').map(str::to_owned).collect(),
            row: 0,
            col: 0,
            path: None,
            crlf,
            eol,
            saved: text.to_owned(),
            undo: Vec::new(),
            redo: Vec::new(),
            change: None,
        }
    }

    // Motion/range offsets count Unicode scalar values, with one character per LF.
    // The final file newline is metadata, not an additional editable row.
    pub fn body(&self) -> String {
        self.lines.join("\n")
    }

    pub fn text(&self) -> String {
        let separator = if self.crlf { "\r\n" } else { "\n" };
        let mut text = self.lines.join(separator);
        if self.eol {
            text.push_str(separator);
        }
        text
    }

    pub fn dirty(&self) -> bool {
        self.text() != self.saved
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            lines: self.lines.clone(),
            row: self.row,
            col: self.col,
            eol: self.eol,
        }
    }

    fn restore(&mut self, snapshot: Snapshot) {
        self.lines = snapshot.lines;
        self.row = snapshot.row;
        self.col = snapshot.col;
        self.eol = snapshot.eol;
    }

    fn checkpoint(&mut self) {
        if self.change.is_none() {
            self.undo.push(self.snapshot());
            self.redo.clear();
        }
    }

    pub fn begin_change(&mut self) {
        if self.change.is_none() {
            self.change = Some(self.snapshot());
        }
    }

    pub fn end_change(&mut self) -> bool {
        let Some(snapshot) = self.change.take() else {
            return false;
        };
        if self.lines != snapshot.lines || self.eol != snapshot.eol {
            self.undo.push(snapshot);
            self.redo.clear();
            true
        } else {
            false
        }
    }

    pub fn offset_at(&self, row: usize, col: usize) -> usize {
        self.lines
            .iter()
            .take(row)
            .map(|l| l.chars().count() + 1)
            .sum::<usize>()
            + col.min(self.lines[row].chars().count())
    }

    pub fn offset(&self) -> usize {
        self.offset_at(self.row, self.col)
    }

    pub fn set_offset(&mut self, mut offset: usize) {
        for (row, line) in self.lines.iter().enumerate() {
            let length = line.chars().count();
            if offset <= length || row + 1 == self.lines.len() {
                self.row = row;
                self.col = offset.min(length);
                return;
            }
            offset -= length + 1;
        }
    }

    pub fn replace(&mut self, start: usize, end: usize, text: &str) {
        let body = self.body();
        let byte = |n| body.char_indices().nth(n).map_or(body.len(), |(i, _)| i);
        let start = byte(start);
        let end = byte(end).max(start);
        let text = text.replace("\r\n", "\n");
        if body[start..end] == text {
            self.set_offset(body[..start].chars().count() + text.chars().count());
            return;
        }
        self.checkpoint();
        let mut result = body[..start].to_owned();
        result.push_str(&text);
        result.push_str(&body[end..]);
        self.lines = result.split('\n').map(str::to_owned).collect();
        self.set_offset(body[..start].chars().count() + text.chars().count());
    }

    pub fn replace_lines(&mut self, start: usize, end: usize, lines: &[String]) {
        self.checkpoint();
        self.lines.splice(start..end, lines.iter().cloned());
        if self.lines.is_empty() {
            self.lines.push(String::new());
            self.eol = false;
        }
        self.row = start.min(self.lines.len() - 1);
        self.col = 0;
    }

    pub fn put_lines(&mut self, start: usize, lines: &[String]) {
        let empty = self.text().is_empty();
        self.replace_lines(
            if empty { 0 } else { start },
            if empty { 1 } else { start },
            lines,
        );
        if empty {
            self.eol = true;
        }
    }

    pub fn insert(&mut self, text: &str) {
        if !text.is_empty() {
            let pos = self.offset();
            self.replace(pos, pos, text);
        }
    }

    pub fn backspace(&mut self) {
        let pos = self.offset();
        if pos > 0 {
            self.replace(pos - 1, pos, "");
        }
    }

    pub fn delete(&mut self) {
        let pos = self.offset();
        if self.col < self.lines[self.row].chars().count() || self.row + 1 < self.lines.len() {
            self.replace(pos, pos + 1, "");
        }
    }

    pub fn undo(&mut self) {
        self.end_change();
        if let Some(snapshot) = self.undo.pop() {
            self.redo.push(self.snapshot());
            self.restore(snapshot);
        }
    }

    pub fn redo(&mut self) {
        self.end_change();
        if let Some(snapshot) = self.redo.pop() {
            self.undo.push(self.snapshot());
            self.restore(snapshot);
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
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Open a filename before saving.",
            )
        })?;
        let text = self.text();
        write_atomic(path, text.as_bytes())?;
        self.saved = text;
        Ok(())
    }
}

pub fn write_atomic(path: &Path, content: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other("Missing filename."))?;
    for attempt in 0..100 {
        let temporary = parent.join(format!(
            ".{}.fvim-{}-{attempt}",
            name.to_string_lossy(),
            std::process::id()
        ));
        let mut file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
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
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "Temporary file collision.",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grouped_changes_undo_and_redo_as_one_edit() {
        let mut b = Buffer::from_text("éone\nlast\n");
        b.begin_change();
        b.replace(1, 4, "two");
        b.insert("!");
        b.end_change();
        assert_eq!(b.text(), "étwo!\nlast\n");
        b.undo();
        assert_eq!(b.text(), "éone\nlast\n");
        b.redo();
        assert_eq!(b.text(), "étwo!\nlast\n");
        b.undo();
        b.insert("x");
        b.redo();
        assert_eq!(b.text(), "xéone\nlast\n");
    }

    #[test]
    fn offsets_are_unicode_characters_and_trailing_newline_is_not_a_row() {
        let mut b = Buffer::from_text("é界\r\nlast\r\n");
        assert_eq!(b.lines.len(), 2);
        b.set_offset(4);
        assert_eq!((b.row, b.col), (1, 1));
        assert_eq!(b.offset(), 4);
        b.replace(1, 4, "🙂");
        assert_eq!(b.text(), "é🙂ast\r\n");
    }

    #[test]
    fn deleting_all_lines_leaves_an_empty_buffer() {
        let mut b = Buffer::from_text("one\ntwo\n");
        b.replace_lines(0, 2, &[]);
        assert_eq!(b.text(), "");
        assert_eq!(b.lines, [""]);
        b.undo();
        assert_eq!(b.text(), "one\ntwo\n");
    }

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
