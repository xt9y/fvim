use crate::editor::{Editor, Mode};
use crossterm::{
    cursor::{Hide, MoveTo, SetCursorStyle, Show},
    queue,
    style::{Attribute, Print, SetAttribute},
    terminal::{Clear, ClearType},
};
use std::io::{self, Write};
use unicode_width::UnicodeWidthChar;

fn char_width(ch: char, column: usize) -> usize {
    if ch == '\t' {
        4 - column % 4
    } else if ch.is_control() {
        1
    } else {
        ch.width().unwrap_or(1)
    }
}

fn display_column(line: &str, chars: usize) -> usize {
    line.chars()
        .take(chars)
        .fold(0, |col, ch| col + char_width(ch, col))
}

#[derive(Default, PartialEq)]
struct Line {
    text: String,
    // Byte offsets where selection attributes change; ordinary rows have none.
    spans: Vec<(usize, bool)>,
}

impl Line {
    fn visible(line: &str, left: usize, width: usize, selected: impl Fn(usize) -> bool) -> Self {
        let mut out = Self::default();
        let (mut column, mut reverse) = (0, false);
        let right = left.saturating_add(width);
        for (index, ch) in line.chars().enumerate() {
            let size = char_width(ch, column);
            let end = column + size;
            let visible = if size == 0 {
                column > left && column <= right
            } else {
                end > left && column < right
            };
            if visible {
                let highlight = selected(index);
                if highlight != reverse {
                    out.spans.push((out.text.len(), highlight));
                    reverse = highlight;
                }
                if size == 0 || (column >= left && end <= right && ch != '\t') {
                    out.text.push(if ch.is_control() { '?' } else { ch });
                } else {
                    for _ in column.max(left)..end.min(right) {
                        out.text.push(' ');
                    }
                }
            }
            column = end;
            if column > right {
                break;
            }
        }
        out
    }

    fn paint(&self, out: &mut impl Write) -> io::Result<()> {
        let mut start = 0;
        for &(end, reverse) in &self.spans {
            queue!(
                out,
                Print(&self.text[start..end]),
                SetAttribute(if reverse {
                    Attribute::Reverse
                } else {
                    Attribute::Reset
                })
            )?;
            start = end;
        }
        queue!(
            out,
            Print(&self.text[start..]),
            SetAttribute(Attribute::Reset)
        )
    }
}

fn visible_line(line: &str, left: usize, width: usize) -> String {
    Line::visible(line, left, width, |_| false).text
}

pub(crate) struct Update {
    clear: bool,
    cursor: (u16, u16, bool),
    cursor_changed: bool,
    style_changed: bool,
}

#[derive(Default)]
pub struct Renderer {
    top: usize,
    left: usize,
    size: Option<(u16, u16)>,
    lines: Vec<Line>,
    changed: Vec<usize>,
    cursor: Option<(u16, u16, bool)>,
}

impl Renderer {
    pub fn invalidate(&mut self) {
        self.size = None;
        self.cursor = None;
    }

    pub(crate) fn prepare(&mut self, editor: &Editor, size: (u16, u16)) -> Option<Update> {
        let (width, height) = size;
        if width == 0 || height < 3 {
            self.size = None;
            self.lines.clear();
            self.cursor = None;
            return None;
        }
        let clear = self.size != Some(size);
        let rows = usize::from(height - 2);
        let columns = usize::from(width);
        let buffer = &editor.buffer;
        if buffer.row < self.top {
            self.top = buffer.row;
        } else if buffer.row >= self.top + rows {
            self.top = buffer.row + 1 - rows;
        }
        let column = display_column(&buffer.lines[buffer.row], buffer.col);
        if column < self.left {
            self.left = column;
        } else if column >= self.left + columns {
            self.left = column + 1 - columns;
        }

        let mut frame = Vec::with_capacity(usize::from(height));
        for screen_row in 0..rows {
            let row = self.top + screen_row;
            let line = if let Some(text) = buffer.lines.get(row) {
                if text.is_empty() && editor.selected(row, 0) {
                    Line {
                        text: " ".into(),
                        spans: vec![(0, true)],
                    }
                } else {
                    Line::visible(text, self.left, columns, |col| editor.selected(row, col))
                }
            } else {
                Line {
                    text: "~".into(),
                    spans: Vec::new(),
                }
            };
            frame.push(line);
        }
        let name = buffer
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|p| p.to_string_lossy())
            .unwrap_or_else(|| "[No Name]".into());
        let status = format!(
            " {} {}{} | {}:{} | Ctrl-S save  Ctrl-Q quit",
            editor.mode_name(),
            name,
            if buffer.dirty() { " [+]" } else { "" },
            buffer.row + 1,
            buffer.col + 1
        );
        frame.push(Line {
            text: visible_line(&status, 0, columns),
            spans: vec![(0, true)],
        });
        frame.push(Line::visible(&editor.command_line(), 0, columns, |_| false));
        self.changed.clear();
        for (row, line) in frame.iter().enumerate() {
            if clear || self.lines.get(row) != Some(line) {
                self.changed.push(row);
            }
        }
        let (x, y) = if let Some(p) = &editor.prompt {
            let col = display_column(&format!("{}{}", p.kind, p.text), p.text.chars().count() + 1)
                .min(columns - 1);
            (col as u16, height - 1)
        } else {
            ((column - self.left) as u16, (buffer.row - self.top) as u16)
        };
        let cursor = (x, y, editor.mode == Mode::Insert);
        let update = Update {
            clear,
            cursor,
            cursor_changed: self.cursor != Some(cursor),
            style_changed: self.cursor.is_none_or(|old| old.2 != cursor.2),
        };
        self.cursor = Some(cursor);
        self.size = Some(size);
        self.lines = frame;
        Some(update)
    }

    pub fn draw(
        &mut self,
        editor: &Editor,
        size: (u16, u16),
        out: &mut impl Write,
    ) -> io::Result<()> {
        let Some(update) = self.prepare(editor, size) else {
            return Ok(());
        };
        let repaint = !self.changed.is_empty();
        if repaint {
            queue!(out, Hide)?;
        }
        if update.clear {
            queue!(out, SetAttribute(Attribute::Reset), Clear(ClearType::All))?;
        }
        for &row in &self.changed {
            queue!(
                out,
                MoveTo(0, row as u16),
                SetAttribute(Attribute::Reset),
                Clear(ClearType::CurrentLine)
            )?;
            self.lines[row].paint(out)?;
        }
        if update.style_changed {
            queue!(
                out,
                if update.cursor.2 {
                    SetCursorStyle::SteadyBar
                } else {
                    SetCursorStyle::SteadyBlock
                }
            )?;
        }
        if update.cursor_changed || repaint {
            queue!(out, MoveTo(update.cursor.0, update.cursor.1))?;
        }
        if repaint {
            queue!(out, Show)?;
        }
        out.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;
    use crate::editor::{Editor, Mode, Visual};

    #[test]
    fn cursor_motion_only_changes_status_and_identical_frames_are_quiet() {
        let mut editor = Editor::new(Buffer::from_text("alpha\nbeta\ngamma"));
        let mut renderer = Renderer::default();
        assert!(renderer.prepare(&editor, (80, 24)).unwrap().clear);
        assert_eq!(renderer.changed.len(), 24);
        renderer.prepare(&editor, (80, 24));
        assert!(renderer.changed.is_empty());
        editor.buffer.col = 1;
        let update = renderer.prepare(&editor, (80, 24)).unwrap();
        assert!(!update.clear);
        assert_eq!(renderer.changed, [22]);
        assert!(update.cursor_changed);
    }

    #[test]
    fn invalidation_repaints_an_identical_frame_and_restores_cursor_style() {
        let editor = Editor::new(Buffer::from_text("alpha\nbeta"));
        let mut renderer = Renderer::default();
        renderer.prepare(&editor, (80, 24));
        renderer.prepare(&editor, (80, 24));
        assert!(renderer.changed.is_empty());
        renderer.invalidate();
        let update = renderer.prepare(&editor, (80, 24)).unwrap();
        assert!(update.clear && update.cursor_changed && update.style_changed);
        assert_eq!(renderer.changed.len(), 24);
        assert_eq!(update.cursor, (0, 0, false));
        renderer.prepare(&editor, (80, 24));
        assert!(renderer.changed.is_empty());
    }

    #[test]
    fn text_selection_scrolling_and_resize_invalidate_visible_rows() {
        let mut editor = Editor::new(Buffer::from_text("alpha\nbeta\ngamma\ndelta"));
        let mut renderer = Renderer::default();
        renderer.prepare(&editor, (8, 5));
        editor.mode = Mode::Visual(Visual::Character);
        renderer.prepare(&editor, (8, 5));
        assert!(renderer.changed.contains(&0));
        editor.mode = Mode::Normal;
        editor.buffer.row = 3;
        renderer.prepare(&editor, (8, 5));
        assert_eq!(renderer.top, 1);
        assert!(renderer.changed.contains(&0));
        assert!(renderer.prepare(&editor, (9, 6)).unwrap().clear);
        assert_eq!(renderer.changed.len(), 6);
        assert!(renderer.prepare(&editor, (0, 1)).is_none());
        assert!(renderer.prepare(&editor, (9, 6)).unwrap().clear);
    }

    #[cfg(unix)]
    #[test]
    fn cursor_only_output_is_small_and_deleted_text_is_cleared() {
        let mut editor = Editor::new(Buffer::from_text(&"alpha beta gamma\n".repeat(100)));
        let mut renderer = Renderer::default();
        let mut output = Vec::new();
        renderer.draw(&editor, (80, 24), &mut output).unwrap();
        output.clear();
        editor.buffer.col = 1;
        renderer.draw(&editor, (80, 24), &mut output).unwrap();
        assert!(
            output.len() < 200,
            "Cursor motion emitted {} bytes",
            output.len()
        );
        assert!(!output.windows(4).any(|part| part == b"\x1b[2J"));
        output.clear();
        renderer.draw(&editor, (80, 24), &mut output).unwrap();
        assert!(output.is_empty());
        editor.buffer.replace_lines(0, 1, &["x".into()]);
        renderer.draw(&editor, (80, 24), &mut output).unwrap();
        assert!(output.windows(4).any(|part| part == b"\x1b[2K"));
    }

    #[test]
    fn rendering_sanitizes_control_sequences_and_clips_unicode_cells() {
        assert_eq!(visible_line("a\u{1b}[31mb", 0, 40), "a?[31mb");
        assert_eq!(visible_line("a\tb", 0, 8), "a   b");
        assert_eq!(display_column("a界e\u{301}\t", 5), 8);
        assert_eq!(visible_line("a界b", 2, 2), " b");
        assert_eq!(visible_line("e\u{301}", 0, 1), "e\u{301}");
    }
}
