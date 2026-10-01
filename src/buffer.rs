use std::borrow::Cow;
use std::collections::VecDeque;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

fn next_revision() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

struct Position {
    row: usize,
    col: usize,
    eol: bool,
}

struct Region {
    row: usize,
    inserted: usize,
    removed: VecDeque<String>,
}

struct Change {
    before: Position,
    region: Option<Region>,
}

pub struct Buffer {
    pub revision: u64,
    pub save_generation: u64,
    pub lines: Vec<String>,
    pub row: usize,
    pub col: usize,
    pub path: Option<PathBuf>,
    crlf: bool,
    eol: bool,
    saved: Vec<String>,
    saved_eol: bool,
    saved_format: bool,
    different: usize,
    lengths: Vec<usize>,
    offsets: Vec<usize>,
    undo: Vec<Change>,
    redo: Vec<Change>,
    change: Option<Change>,
}

impl Buffer {
    pub fn open(path: Option<PathBuf>) -> io::Result<Self> {
        if path.as_ref().is_some_and(|p| p.is_dir()) {
            return Err(io::Error::other("Cannot open a directory as a text buffer"));
        }
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
        let saved_format = !crlf
            || text
                .split_inclusive('\n')
                .all(|line| !line.ends_with('\n') || line.ends_with("\r\n"));
        let normalized = if crlf {
            Cow::Owned(text.replace("\r\n", "\n"))
        } else {
            Cow::Borrowed(text)
        };
        let eol = normalized.ends_with('\n');
        let body = if eol {
            &normalized[..normalized.len() - 1]
        } else {
            &normalized
        };
        let lines: Vec<_> = body.split('\n').map(str::to_owned).collect();
        let mut buffer = Self {
            revision: next_revision(),
            save_generation: 0,
            saved: lines.clone(),
            lines,
            row: 0,
            col: 0,
            path: None,
            crlf,
            eol,
            saved_eol: eol,
            saved_format,
            different: 0,
            lengths: Vec::new(),
            offsets: Vec::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            change: None,
        };
        buffer.reindex();
        buffer
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
        self.different != 0 || self.eol != self.saved_eol || !self.saved_format
    }

    fn position(&self) -> Position {
        Position {
            row: self.row,
            col: self.col,
            eol: self.eol,
        }
    }

    fn reindex(&mut self) {
        self.lengths = self.lines.iter().map(|line| line.chars().count()).collect();
        self.offsets.clear();
        self.offsets.resize(self.lines.len() + 1, 0);
        // Fenwick sums include one LF per row; the last LF is never editable.
        for i in 1..self.offsets.len() {
            self.offsets[i] += self.lengths[i - 1] + 1;
            let parent = i + (1usize << i.trailing_zeros());
            if parent < self.offsets.len() {
                self.offsets[parent] += self.offsets[i];
            }
        }
        self.different = (0..self.lines.len().max(self.saved.len()))
            .filter(|&row| self.lines.get(row) != self.saved.get(row))
            .count();
    }

    fn update_line(&mut self, row: usize, was_different: bool) {
        self.different -= usize::from(was_different);
        self.different += usize::from(self.lines.get(row) != self.saved.get(row));
        let old = self.lengths[row];
        let new = self.lines[row].chars().count();
        self.lengths[row] = new;
        let mut i = row + 1;
        while i < self.offsets.len() {
            self.offsets[i] = self.offsets[i] - old + new;
            i += 1usize << i.trailing_zeros();
        }
    }

    pub fn begin_change(&mut self) {
        if self.change.is_none() {
            self.change = Some(Change {
                before: self.position(),
                region: None,
            });
        }
    }

    // Expand the original undo span only when an edit touches new rows.
    // Repeated keystrokes on a row retain just its original text.
    fn record_span(&mut self, start: usize, end: usize) {
        let change = self.change.as_mut().unwrap();
        if let Some(region) = &mut change.region {
            let first = start.min(region.row);
            let last = end.max(region.row + region.inserted);
            if first < region.row {
                for line in self.lines[first..region.row].iter().rev() {
                    region.removed.push_front(line.clone());
                }
            }
            region.removed.extend(
                self.lines[region.row + region.inserted..last]
                    .iter()
                    .cloned(),
            );
            region.row = first;
            region.inserted = last - first;
        } else {
            change.region = Some(Region {
                row: start,
                inserted: end - start,
                removed: self.lines[start..end].iter().cloned().collect(),
            });
        }
    }

    pub fn end_change(&mut self) -> bool {
        let Some(change) = self.change.take() else {
            return false;
        };
        let same = change.region.as_ref().is_none_or(|region| {
            self.lines[region.row..region.row + region.inserted]
                .iter()
                .eq(region.removed.iter())
        });
        if !same || self.eol != change.before.eol {
            self.undo.push(change);
            self.redo.clear();
            true
        } else {
            false
        }
    }

    pub fn offset_at(&self, row: usize, col: usize) -> usize {
        let mut sum = col.min(self.lengths[row]);
        let mut i = row;
        while i != 0 {
            sum += self.offsets[i];
            i -= 1usize << i.trailing_zeros();
        }
        sum
    }

    pub fn offset(&self) -> usize {
        self.offset_at(self.row, self.col)
    }

    fn position_at(&self, offset: usize) -> (usize, usize) {
        let (mut row, mut sum) = (0, 0);
        let mut step = self.offsets.len().next_power_of_two() / 2;
        while step != 0 {
            let next = row + step;
            if next < self.offsets.len() && sum + self.offsets[next] <= offset {
                sum += self.offsets[next];
                row = next;
            }
            step /= 2;
        }
        if row >= self.lines.len() {
            row = self.lines.len() - 1;
            (row, self.lengths[row])
        } else {
            (row, (offset - sum).min(self.lengths[row]))
        }
    }

    pub fn set_offset(&mut self, offset: usize) {
        (self.row, self.col) = self.position_at(offset);
    }

    fn byte_at(&self, row: usize, col: usize) -> usize {
        let line = &self.lines[row];
        if self.lengths[row] == line.len() {
            col.min(line.len())
        } else {
            line.char_indices().nth(col).map_or(line.len(), |(i, _)| i)
        }
    }

    pub fn chars_forward(&self, offset: usize) -> impl Iterator<Item = char> + '_ {
        let (row, col) = self.position_at(offset);
        self.lines[row][self.byte_at(row, col)..].chars().chain(
            self.lines[row + 1..]
                .iter()
                .flat_map(|line| std::iter::once('\n').chain(line.chars())),
        )
    }

    pub fn chars_backward(&self, offset: usize) -> impl Iterator<Item = char> + '_ {
        let (row, col) = self.position_at(offset);
        self.lines[row][..self.byte_at(row, col)]
            .chars()
            .rev()
            .chain(
                self.lines[..row]
                    .iter()
                    .rev()
                    .flat_map(|line| std::iter::once('\n').chain(line.chars().rev())),
            )
    }

    pub fn body_len(&self) -> usize {
        let last = self.lines.len() - 1;
        self.offset_at(last, self.lengths[last])
    }

    fn splice<I>(&mut self, start: usize, end: usize, lines: I) -> VecDeque<String>
    where
        I: IntoIterator<Item = String>,
        I::IntoIter: ExactSizeIterator,
    {
        self.revision = next_revision();
        let lines = lines.into_iter();
        let same_length = end - start == lines.len();
        let old_different = if same_length {
            (start..end)
                .filter(|&row| self.lines.get(row) != self.saved.get(row))
                .count()
        } else {
            0
        };
        let removed = self.lines.splice(start..end, lines).collect();
        if same_length {
            // Apply each row's old mismatch once, then compare the new contents.
            self.different -= old_different;
            for row in start..end {
                self.update_line(row, false);
            }
        } else {
            self.reindex();
        }
        removed
    }

    pub fn replace(&mut self, start: usize, end: usize, text: &str) {
        let (first, from) = self.position_at(start);
        let (last, to) = self.position_at(end.max(start));
        let start = self.offset_at(first, from);
        let normalized;
        let text = if text.contains("\r\n") {
            normalized = text.replace("\r\n", "\n");
            &normalized
        } else {
            text
        };
        let next = start + text.chars().count();
        let from = self.byte_at(first, from);
        let to = self.byte_at(last, to);
        if first == last && !text.contains('\n') {
            if self.lines[first][from..to] == *text {
                self.set_offset(next);
                return;
            }
            let standalone = self.change.is_none();
            self.begin_change();
            self.record_span(first, last + 1);
            let was_different = self.lines.get(first) != self.saved.get(first);
            self.revision = next_revision();
            self.lines[first].replace_range(from..to, text);
            self.update_line(first, was_different);
            self.set_offset(next);
            if standalone {
                self.end_change();
            }
            return;
        }
        let mut replacement: Vec<String> = text.split('\n').map(str::to_owned).collect();
        replacement[0].insert_str(0, &self.lines[first][..from]);
        replacement
            .last_mut()
            .unwrap()
            .push_str(&self.lines[last][to..]);
        self.replace_lines(first, last + 1, &replacement);
        self.set_offset(next);
    }

    pub fn replace_lines(&mut self, start: usize, end: usize, lines: &[String]) {
        let standalone = self.change.is_none();
        self.begin_change();
        self.record_span(start, end);
        let mut replacement = lines.to_vec();
        if end - start == self.lines.len() && replacement.is_empty() {
            replacement.push(String::new());
            self.eol = false;
        }
        let region = self.change.as_mut().unwrap().region.as_mut().unwrap();
        region.inserted = region.inserted - (end - start) + replacement.len();
        self.splice(start, end, replacement);
        self.row = start.min(self.lines.len() - 1);
        self.col = 0;
        if standalone {
            self.end_change();
        }
    }

    pub fn put_lines(&mut self, start: usize, lines: &[String]) {
        let standalone = self.change.is_none();
        self.begin_change();
        let empty = self.lines.len() == 1 && self.lines[0].is_empty() && !self.eol;
        self.replace_lines(
            if empty { 0 } else { start },
            if empty { 1 } else { start },
            lines,
        );
        if empty {
            self.eol = true;
        }
        if standalone {
            self.end_change();
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

    fn exchange(&mut self, change: Change) -> Change {
        let before = self.position();
        let region = change.region.map(|region| {
            let inserted = region.removed.len();
            let removed = self.splice(region.row, region.row + region.inserted, region.removed);
            Region {
                row: region.row,
                inserted,
                removed,
            }
        });
        self.row = change.before.row;
        self.col = change.before.col;
        self.eol = change.before.eol;
        Change { before, region }
    }

    pub fn undo(&mut self) {
        self.end_change();
        if let Some(change) = self.undo.pop() {
            let inverse = self.exchange(change);
            self.redo.push(inverse);
        }
    }

    pub fn redo(&mut self) {
        self.end_change();
        if let Some(change) = self.redo.pop() {
            let inverse = self.exchange(change);
            self.undo.push(inverse);
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
        self.saved = self.lines.clone();
        self.saved_eol = self.eol;
        self.saved_format = true;
        self.different = 0;
        self.save_generation += 1;
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
    fn reloaded_buffers_have_distinct_document_revisions() {
        let first = Buffer::from_text("before");
        let second = Buffer::from_text("after");
        assert_ne!(first.revision, second.revision);
    }

    #[test]
    fn mixed_line_endings_are_dirty_until_normalized_save() {
        for (text, expected) in [
            ("one\r\ntwo\n", "one\r\ntwo\r\n"),
            ("\none\r\n", "\r\none\r\n"),
            ("one\r\ntwo", "one\r\ntwo"),
        ] {
            let mut b = Buffer::from_text(text);
            assert_eq!(b.text(), expected);
            assert_eq!(b.dirty(), text != expected);
            b.insert("x");
            b.undo();
            assert_eq!(b.dirty(), text != expected);
        }
    }

    #[test]
    fn grouped_splices_expand_across_rows_and_cancel_exactly() {
        let mut b = Buffer::from_text("a\nb\nc\nd\n");
        b.begin_change();
        b.replace_lines(2, 3, &["C".into(), "extra".into()]);
        b.replace_lines(0, 1, &["A".into()]);
        b.replace_lines(4, 5, &[]);
        assert!(b.end_change());
        assert_eq!(b.text(), "A\nb\nC\nextra\n");
        b.undo();
        assert_eq!(b.text(), "a\nb\nc\nd\n");
        assert!(!b.dirty());
        b.redo();
        assert_eq!(b.text(), "A\nb\nC\nextra\n");
        b.begin_change();
        b.replace_lines(0, 1, &["changed".into()]);
        b.replace_lines(3, 4, &["last".into()]);
        b.replace_lines(0, 1, &["A".into()]);
        b.replace_lines(3, 4, &["extra".into()]);
        assert!(!b.end_change());
        b.undo();
        assert_eq!(b.text(), "a\nb\nc\nd\n");
    }

    #[test]
    fn local_changes_match_a_string_model_through_undo_and_offsets() {
        let initial = "éone\n\n界last";
        let mut b = Buffer::from_text(initial);
        let mut model = initial.to_owned();
        let mut seed = 42u64;
        let mut random = |limit: usize| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            (seed >> 32) as usize % limit
        };
        for _ in 0..100 {
            let previous = model.clone();
            b.begin_change();
            for _ in 0..3 {
                let length = model.chars().count();
                let start = random(length + 1);
                let end = (start + random(5)).min(length);
                let text = ["", "x", "🙂", "\n", "é\n界"][random(5)];
                let byte = |n| model.char_indices().nth(n).map_or(model.len(), |(i, _)| i);
                let range = byte(start)..byte(end);
                model.replace_range(range, text);
                b.replace(start, end, text);
                assert_eq!(b.body(), model);
                assert_eq!(b.dirty(), model != initial);
                for offset in 0..=model.chars().count() {
                    b.set_offset(offset);
                    assert_eq!(b.offset(), offset);
                    let prefix: String = model.chars().take(offset).collect();
                    assert_eq!(b.row, prefix.chars().filter(|&ch| ch == '\n').count());
                    assert_eq!(b.col, prefix.rsplit('\n').next().unwrap().chars().count());
                }
            }
            let changed = b.end_change();
            assert_eq!(changed, model != previous);
            if changed {
                b.undo();
                assert_eq!(b.body(), previous);
                assert_eq!(b.dirty(), previous != initial);
                b.redo();
                assert_eq!(b.body(), model);
            }
        }
    }

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
