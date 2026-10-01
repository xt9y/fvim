mod status;
mod text;

use crate::config::{Highlight, Settings};
use crate::editor::{Editor, Mode};
use crossterm::{
    cursor::{Hide, MoveTo, SetCursorStyle, Show},
    queue,
    terminal::{Clear, ClearType},
};
use status::statusline;
use std::collections::HashMap;
use std::io::{self, Write};
pub(crate) use text::display_column;
use text::{char_width, clipped, paint_style, Line};
#[cfg(test)]
use unicode_width::UnicodeWidthStr;

// Only wrapping visits the entire current/visible row; the default no-wrap path clips early.
fn segments(
    line: &str,
    width: usize,
    settings: &Settings,
    insert_end: bool,
) -> Vec<(usize, usize)> {
    let mut boundaries = Vec::new();
    let mut column = 0;
    for ch in line.chars() {
        let end = column + char_width(ch, column, settings.tabstop);
        boundaries.push((column, end, ch.is_whitespace()));
        column = end;
    }
    let mut chunks = Vec::new();
    let mut start = 0;
    let mut index = 0;
    while start < column {
        let mut end = (start + width).min(column);
        let mut whitespace = None;
        while index < boundaries.len() && boundaries[index].0 < end {
            let (a, b, space) = boundaries[index];
            if b > end && a > start {
                end = a;
                break;
            }
            if space && b <= end {
                whitespace = Some(b);
            }
            index += 1;
        }
        if settings.linebreak && end < column {
            if let Some(boundary) = whitespace.filter(|&p| p > start) {
                end = boundary;
            }
        }
        chunks.push((start, end));
        start = end;
        // A word after a linebreak may have been examined already.
        while index > 0 && boundaries[index - 1].0 >= start {
            index -= 1;
        }
    }
    if chunks.is_empty() || (insert_end && chunks.last().is_some_and(|p| p.1 - p.0 == width)) {
        chunks.push((column, column));
    }
    chunks
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
    segment: usize,
    left: usize,
    size: Option<(u16, u16)>,
    lines: Vec<Line>,
    changed: Vec<usize>,
    cursor: Option<(u16, u16, bool)>,
    cursor_style: String,
    theme: Option<(bool, HashMap<String, Highlight>)>,
}

impl Renderer {
    pub fn invalidate(&mut self) {
        self.size = None;
        self.cursor = None;
    }

    pub(crate) fn prepare(&mut self, editor: &Editor, size: (u16, u16)) -> Option<Update> {
        self.prepare_inner(editor, size, false)
    }

    fn prepare_inner(&mut self, editor: &Editor, size: (u16, u16), pane: bool) -> Option<Update> {
        let (width, height) = size;
        if width == 0 || height == 0 {
            self.size = None;
            self.lines.clear();
            self.cursor = None;
            return None;
        }
        let s = &editor.settings;
        let theme_changed = self
            .theme
            .as_ref()
            .is_none_or(|(rgb, hl)| *rgb != s.termguicolors || hl != &s.highlights);
        let clear = self.size != Some(size) || theme_changed;
        if theme_changed {
            self.theme = Some((s.termguicolors, s.highlights.clone()));
        }
        let normal = s.highlight("Normal");
        let visual = s.highlight("Visual");
        let mode_message = if s.showmode && editor.mode != Mode::Normal {
            format!("-- {} --", editor.mode_name())
        } else {
            String::new()
        };
        let command = if editor.prompt.is_some() || !editor.message.is_empty() {
            editor.command_line()
        } else {
            mode_message.clone()
        };
        let commands = if pane {
            0
        } else {
            s.cmdheight
                .max(usize::from(!command.is_empty()))
                .min(usize::from(height))
        };
        let status = usize::from(s.laststatus >= 2 && usize::from(height) > commands + 1);
        let rows = usize::from(height) - commands - status;
        let columns = usize::from(width);
        let b = &editor.buffer;
        let digits = b.lines.len().to_string().len();
        let number_width = if s.number || s.relativenumber {
            s.numberwidth.max(digits + 1)
        } else {
            0
        };
        let signs = if s.signcolumn { 2 } else { 0 };
        let gutter = (number_width + signs).min(columns.saturating_sub(1));
        let text_width = columns - gutter;
        let column = display_column(&b.lines[b.row], b.col, s.tabstop);
        let insertion = editor.mode == Mode::Insert && b.col == b.lines[b.row].chars().count();
        let row_segments =
            |row: usize| segments(&b.lines[row], text_width, s, insertion && row == b.row);
        let mut cursor_y = 0;
        let mut cursor_x = 0;
        if rows > 0 {
            let margin = s.scrolloff.min((rows - 1) / 2);
            if !s.wrap {
                self.segment = 0;
                if b.row < self.top + margin {
                    self.top = b.row.saturating_sub(margin);
                } else if b.row >= self.top + rows - margin {
                    self.top = (b.row + margin + 1).saturating_sub(rows);
                }
                self.top = self.top.min(b.lines.len().saturating_sub(rows));
                let side = s.sidescrolloff.min((text_width - 1) / 2);
                if column < self.left + side {
                    self.left = column.saturating_sub(side);
                } else if column >= self.left + text_width - side {
                    let desired = (column + side + 1).saturating_sub(text_width);
                    self.left = if s.sidescroll == 0 {
                        column.saturating_sub(text_width / 2)
                    } else {
                        desired.max(self.left + s.sidescroll).min(column)
                    };
                }
                cursor_y = b.row - self.top;
                cursor_x = column - self.left;
            } else {
                self.left = 0;
                let current = row_segments(b.row);
                let cursor_segment = current.iter().rposition(|p| p.0 <= column).unwrap_or(0);
                let cursor = (b.row, cursor_segment);
                let prev = |mut p: (usize, usize)| {
                    if p.1 > 0 {
                        p.1 -= 1;
                    } else if p.0 > 0 {
                        p.0 -= 1;
                        p.1 = row_segments(p.0).len() - 1;
                    }
                    p
                };
                let next = |mut p: (usize, usize)| {
                    if p.1 + 1 < row_segments(p.0).len() {
                        p.1 += 1;
                    } else if p.0 + 1 < b.lines.len() {
                        p = (p.0 + 1, 0);
                    }
                    p
                };
                let back = |mut p, n| {
                    for _ in 0..n {
                        p = prev(p);
                    }
                    p
                };
                let mut top = (self.top.min(b.lines.len() - 1), self.segment);
                top.1 = top.1.min(row_segments(top.0).len() - 1);
                let mut position = top;
                let mut distance = 0;
                while position < cursor && distance < rows {
                    position = next(position);
                    distance += 1;
                }
                if cursor < top || distance < margin {
                    top = back(cursor, margin);
                } else if distance >= rows - margin {
                    top = back(cursor, rows - margin - 1);
                }
                let last_row = b.lines.len() - 1;
                let max_top = back((last_row, row_segments(last_row).len() - 1), rows - 1);
                top = top.min(max_top);
                self.top = top.0;
                self.segment = top.1;
                let mut p = top;
                while p < cursor && cursor_y < rows - 1 {
                    p = next(p);
                    cursor_y += 1;
                }
                cursor_x = column - current[cursor_segment].0;
            }
        }
        let mut frame = Vec::with_capacity(usize::from(height));
        let mut row = self.top;
        let mut segment = self.segment;
        let mut chunks = if s.wrap && row < b.lines.len() {
            row_segments(row)
        } else {
            vec![]
        };
        for _ in 0..rows {
            let mut line = Line::default();
            if let Some(text) = b.lines.get(row) {
                if signs > 0 {
                    line.add(&" ".repeat(signs.min(gutter)), s.highlight("SignColumn"));
                }
                if number_width > 0 && gutter > signs {
                    let number = if segment > 0 {
                        " ".repeat(number_width)
                    } else if s.relativenumber && (row != b.row || !s.number) {
                        format!("{:>w$} ", row.abs_diff(b.row), w = number_width - 1)
                    } else if s.relativenumber {
                        format!("{:<w$} ", row + 1, w = number_width - 1)
                    } else {
                        format!("{:>w$} ", row + 1, w = number_width - 1)
                    };
                    line.add(
                        &clipped(&number, gutter - signs),
                        s.highlight(if row == b.row {
                            "CursorLineNr"
                        } else {
                            "LineNr"
                        }),
                    );
                }
                let (left, visible_width) = if s.wrap {
                    (chunks[segment].0, chunks[segment].1 - chunks[segment].0)
                } else {
                    (self.left, text_width)
                };
                if text.is_empty() && editor.selected(row, 0) {
                    line.add(" ", visual);
                } else {
                    line.append(Line::visible(
                        text,
                        left,
                        visible_width,
                        s.tabstop,
                        normal,
                        visual,
                        |col| editor.selected(row, col),
                    ));
                }
                if s.wrap && segment + 1 < chunks.len() {
                    segment += 1;
                } else {
                    row += 1;
                    segment = 0;
                    if s.wrap && row < b.lines.len() {
                        chunks = row_segments(row);
                    }
                }
            } else {
                line.add(&s.eob, s.highlight("EndOfBuffer"));
                row += 1;
            }
            frame.push(line);
        }
        if status > 0 {
            let mut line = Line::default();
            line.add(&statusline(editor, columns), s.highlight("StatusLine"));
            frame.push(line);
        }
        if commands > 0 {
            frame.push(Line::visible(
                &command,
                0,
                columns,
                s.tabstop,
                if command == mode_message {
                    s.highlight("ModeMsg")
                } else {
                    normal
                },
                visual,
                |_| false,
            ));
            for _ in 1..commands {
                frame.push(Line::default());
            }
        }
        self.changed.clear();
        for (row, line) in frame.iter().enumerate() {
            if clear || self.lines.get(row) != Some(line) {
                self.changed.push(row);
            }
        }
        let (x, y) = if let Some(p) = editor.prompt.as_ref().filter(|_| !pane) {
            (
                display_column(
                    &format!("{}{}", p.kind, p.text),
                    p.text.chars().count() + 1,
                    s.tabstop,
                )
                .min(columns - 1),
                rows + status,
            )
        } else {
            (
                (gutter + cursor_x).min(columns - 1),
                cursor_y.min(usize::from(height) - 1),
            )
        };
        let cursor = (x as u16, y as u16, editor.mode == Mode::Insert);
        let shape = if cursor.2 {
            &s.insert_cursor
        } else {
            &s.normal_cursor
        };
        let update = Update {
            clear,
            cursor,
            cursor_changed: self.cursor != Some(cursor),
            style_changed: self.cursor.is_none() || &self.cursor_style != shape,
        };
        self.cursor = Some(cursor);
        self.cursor_style.clone_from(shape);
        self.size = Some(size);
        self.lines = frame;
        Some(update)
    }

    pub fn draw_workspace(
        &mut self,
        workspace: &mut crate::workspace::Workspace,
        size: (u16, u16),
        out: &mut impl Write,
    ) -> io::Result<()> {
        if workspace.windows.iter().flatten().count() == 1 && workspace.picker.is_none() {
            let editor = workspace.editor_mut();
            editor.page_rows = (size.1 as usize)
                .saturating_sub(editor.settings.cmdheight)
                .saturating_sub(usize::from(editor.settings.laststatus >= 2));
            return self.draw(editor, size, out);
        }
        let (width, height) = size;
        if width == 0 || height == 0 {
            self.invalidate();
            return Ok(());
        }
        let s = &workspace.settings;
        let normal = s.highlight("Normal");
        let active = workspace.active;
        let active_buffer = workspace.windows[active].as_ref().unwrap().buffer;
        let saved = (workspace.editor().buffer.row, workspace.editor().buffer.col);
        let command = workspace.editor().command_line();
        let command = if command.is_empty() && s.showmode && workspace.editor().mode != Mode::Normal
        {
            format!("-- {} --", workspace.editor().mode_name())
        } else {
            command
        };
        let commands = s
            .cmdheight
            .max(usize::from(!command.is_empty()))
            .max(1)
            .min(height as usize);
        let rects = workspace.rectangles(width, height.saturating_sub(commands as u16));
        let mut pieces: Vec<Vec<(u16, Line)>> = (0..height).map(|_| vec![]).collect();
        let mut cursor = (0, 0, false);
        let shape = if workspace.editor().mode == Mode::Insert {
            s.insert_cursor.clone()
        } else {
            s.normal_cursor.clone()
        };
        let mut active_rows = 0;
        for (id, r) in rects {
            let w = workspace.windows[id].as_mut().unwrap();
            let e = &mut workspace.buffers[w.buffer];
            let position = (e.buffer.row, e.buffer.col);
            let mode = e.mode;
            let (row, col) = if id == active { saved } else { (w.row, w.col) };
            e.buffer.row = row.min(e.buffer.lines.len() - 1);
            e.buffer.col = col.min(
                e.buffer.lines[e.buffer.row]
                    .chars()
                    .count()
                    .saturating_sub(usize::from(e.mode != Mode::Insert)),
            );
            if id != active {
                e.mode = Mode::Normal;
            }
            let border = u16::from(r.x + r.width < width && r.width > 1);
            let pane_width = r.width - border;
            e.page_rows =
                r.height
                    .saturating_sub(u16::from(e.settings.laststatus >= 2)) as usize;
            if id == active {
                active_rows = e.page_rows;
            }
            if let Some(u) = w.renderer.prepare_inner(e, (pane_width, r.height), true) {
                if id == active {
                    cursor = (r.x + u.cursor.0, r.y + u.cursor.1, u.cursor.2);
                }
                for (row, line) in w.renderer.lines.iter().enumerate() {
                    let mut line = line.clone();
                    let cells = unicode_width::UnicodeWidthStr::width(line.text.as_str());
                    line.add(
                        &" ".repeat((pane_width as usize).saturating_sub(cells)),
                        normal,
                    );
                    if border > 0 {
                        line.add("│", workspace.settings.highlight("LineNr"));
                    }
                    pieces[r.y as usize + row].push((r.x, line));
                }
            }
            e.buffer.row = position.0;
            e.buffer.col = position.1;
            e.mode = mode;
        }
        workspace.buffers[active_buffer].page_rows = active_rows;
        workspace.buffers[active_buffer].buffer.row = saved.0;
        workspace.buffers[active_buffer].buffer.col = saved.1;
        let mut frame = Vec::new();
        for mut row in pieces {
            row.sort_by_key(|p| p.0);
            let mut line = Line::default();
            for (_, piece) in row {
                line.append(piece);
            }
            frame.push(line);
        }
        let command_row = height as usize - commands;
        frame[command_row] = Line::visible(
            &command,
            0,
            width as usize,
            workspace.settings.tabstop,
            normal,
            normal,
            |_| false,
        );
        if let Some(p) = &workspace.editor().prompt {
            cursor = (
                display_column(
                    &format!("{}{}", p.kind, p.text),
                    p.text.chars().count() + 1,
                    workspace.settings.tabstop,
                )
                .min(width as usize - 1) as u16,
                command_row as u16,
                false,
            );
        }
        if let Some(p) = &workspace.picker {
            let title = format!("{} > {}", if p.grep { "Grep" } else { "Files" }, p.query);
            frame[0] = Line::visible(
                &title,
                0,
                width as usize,
                workspace.settings.tabstop,
                workspace.settings.highlight("StatusLine"),
                normal,
                |_| false,
            );
            let available = height.saturating_sub(2) as usize;
            let top = p.selected.saturating_sub(available.saturating_sub(1));
            for row in 0..available {
                let label = p
                    .entries
                    .get(top + row)
                    .map(|e| {
                        format!(
                            "{} {}",
                            if top + row == p.selected { ">" } else { " " },
                            e.label
                        )
                    })
                    .unwrap_or_default();
                frame[row + 1] = Line::visible(
                    &label,
                    0,
                    width as usize,
                    workspace.settings.tabstop,
                    if top + row == p.selected {
                        workspace.settings.highlight("Visual")
                    } else {
                        normal
                    },
                    normal,
                    |_| false,
                );
            }
            frame[height as usize - 1] = Line::visible(
                &format!("{} | Enter open | hh/vv split | Esc close", p.message),
                0,
                width as usize,
                workspace.settings.tabstop,
                normal,
                normal,
                |_| false,
            );
            cursor = (
                display_column(&title, title.chars().count(), workspace.settings.tabstop)
                    .min(width as usize - 1) as u16,
                0,
                true,
            );
        }
        let theme_changed = self.theme.as_ref().is_none_or(|(rgb, hl)| {
            *rgb != workspace.settings.termguicolors || hl != &workspace.settings.highlights
        });
        let clear = self.size != Some(size) || theme_changed;
        self.changed.clear();
        for (row, line) in frame.iter().enumerate() {
            if clear || self.lines.get(row) != Some(line) {
                self.changed.push(row);
            }
        }
        let update = Update {
            clear,
            cursor,
            cursor_changed: self.cursor != Some(cursor),
            style_changed: self.cursor.is_none() || self.cursor_style != shape,
        };
        self.lines = frame;
        self.size = Some(size);
        self.cursor = Some(cursor);
        self.cursor_style = shape;
        self.theme = Some((
            workspace.settings.termguicolors,
            workspace.settings.highlights.clone(),
        ));
        self.paint(update, &workspace.settings, out)
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
        self.paint(update, &editor.settings, out)
    }

    fn paint(
        &mut self,
        update: Update,
        settings: &Settings,
        out: &mut impl Write,
    ) -> io::Result<()> {
        let rgb = settings.termguicolors;
        let normal = settings.highlight("Normal");
        let repaint = !self.changed.is_empty();
        if repaint {
            queue!(out, Hide)?;
        }
        if update.clear {
            paint_style(out, normal, rgb)?;
            queue!(out, Clear(ClearType::All))?;
        }
        for &row in &self.changed {
            queue!(out, MoveTo(0, row as u16))?;
            paint_style(out, normal, rgb)?;
            queue!(out, Clear(ClearType::CurrentLine))?;
            self.lines[row].paint(out, rgb)?;
        }
        if update.style_changed {
            queue!(
                out,
                match self.cursor_style.as_str() {
                    "bar" => SetCursorStyle::SteadyBar,
                    "underline" => SetCursorStyle::SteadyUnderScore,
                    _ => SetCursorStyle::SteadyBlock,
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
    fn paging_uses_the_active_panes_height() {
        let mut w = crate::workspace::Workspace::new(
            Buffer::from_text(&"line\n".repeat(100)),
            std::path::PathBuf::from("."),
        );
        w.command("split").unwrap();
        w.command("split").unwrap();
        w.focus(0);
        Renderer::default()
            .draw_workspace(&mut w, (80, 25), &mut Vec::new())
            .unwrap();
        assert_eq!(w.editor().page_rows, 11);
    }

    #[test]
    fn workspace_splits_render_shared_edits_and_tiny_terminal_bounds() {
        let mut w = crate::workspace::Workspace::new(
            Buffer::from_text("alpha\nbeta"),
            std::path::PathBuf::from("."),
        );
        w.command("vsplit").unwrap();
        w.command("split").unwrap();
        let mut r = Renderer::default();
        let mut output = Vec::new();
        for width in 1..15 {
            for height in 1..8 {
                r.draw_workspace(&mut w, (width, height), &mut output)
                    .unwrap();
                assert!(r.cursor.unwrap().0 < width && r.cursor.unwrap().1 < height);
                assert_eq!(r.lines.len(), height as usize);
                for line in &r.lines {
                    assert!(
                        unicode_width::UnicodeWidthStr::width(line.text.as_str()) <= width as usize
                    );
                }
                output.clear();
            }
        }
        r.draw_workspace(&mut w, (80, 24), &mut output).unwrap();
        assert!(r.lines.iter().any(|l| l.text.matches("alpha").count() == 2));
        output.clear();
        r.draw_workspace(&mut w, (80, 24), &mut output).unwrap();
        assert!(output.is_empty());
    }

    #[test]
    fn native_layout_uses_gutters_full_statusline_and_command_area() {
        let mut e = Editor::new(Buffer::from_text("alpha\nbeta\ngamma"));
        e.buffer.row = 1;
        let mut r = Renderer::default();
        r.prepare(&e, (40, 8));
        // Two sign cells followed by a four-cell number column.
        assert_eq!(r.lines[0].text, "    1 alpha");
        assert_eq!(r.lines[1].text, "  2   beta");
        assert_eq!(r.lines[2].text, "    1 gamma");
        assert_eq!(
            unicode_width::UnicodeWidthStr::width(r.lines[6].text.as_str()),
            40
        );
        assert!(!r.lines[6].text.contains("Ctrl-S"));
        assert!(r.lines[7].text.is_empty());
        e.mode = Mode::Insert;
        r.prepare(&e, (40, 8));
        assert_eq!(r.lines[7].text, "-- INSERT --");
    }

    #[test]
    fn settings_center_scroll_and_wrap_without_losing_the_cursor() {
        let mut e = Editor::new(Buffer::from_text(&"abcdef ghij\n".repeat(100)));
        e.settings.number = false;
        e.settings.relativenumber = false;
        e.settings.signcolumn = false;
        e.buffer.row = 50;
        let mut r = Renderer::default();
        let update = r.prepare(&e, (8, 7)).unwrap();
        assert_eq!(update.cursor.1, 2);
        e.settings.wrap = true;
        e.settings.linebreak = true;
        e.buffer.col = 9;
        let update = r.prepare(&e, (8, 7)).unwrap();
        assert!(update.cursor.0 < 8 && update.cursor.1 < 5);
        assert!(r.lines.iter().any(|line| line.text == "ghij"));
    }

    #[test]
    fn small_terminals_and_wrap_toggles_keep_prompt_and_cursor_in_bounds() {
        let mut e = Editor::new(Buffer::from_text("a\t界e\u{301} wide words\nnext\n"));
        let mut r = Renderer::default();
        for wrap in [true, false, true] {
            e.settings.wrap = wrap;
            e.settings.linebreak = true;
            for width in 1..12 {
                for height in 1..8 {
                    e.buffer.col = 9;
                    e.prompt = None;
                    let u = r.prepare(&e, (width, height)).unwrap();
                    assert!(u.cursor.0 < width && u.cursor.1 < height);
                    assert_eq!(r.lines.len(), usize::from(height));
                    e.settings.cmdheight = 0;
                    e.prompt = Some(crate::editor::Prompt {
                        kind: ':',
                        text: "long界command".into(),
                    });
                    let u = r.prepare(&e, (width, height)).unwrap();
                    assert!(u.cursor.0 < width && u.cursor.1 < height);
                    assert_eq!(r.lines.len(), usize::from(height));
                }
            }
        }
    }

    #[test]
    fn statusline_position_is_visible_in_the_middle_of_a_file() {
        let mut e = Editor::new(Buffer::from_text(&"line\n".repeat(100)));
        e.settings.statusline = "%P".into();
        e.buffer.row = 49;
        let mut r = Renderer::default();
        r.prepare(&e, (24, 8));
        assert_eq!(r.lines[6].text.trim(), "50%");
    }

    #[test]
    fn truncated_statusline_sanitizes_file_control_characters() {
        let mut e = Editor::new(Buffer::from_text("x"));
        e.buffer.path = Some(std::path::PathBuf::from(
            "very/long/path/name/\u{1b}[31mfile.rs",
        ));
        let mut r = Renderer::default();
        r.prepare(&e, (24, 8));
        assert!(!r.lines[6].text.contains('\u{1b}'));
        assert_eq!(r.lines[6].text.width(), 24);
    }

    #[cfg(unix)]
    #[test]
    fn theme_change_repaints_and_indexed_mode_emits_no_rgb_sequences() {
        crossterm::style::force_color_output(true);
        let mut e = Editor::new(Buffer::from_text("alpha"));
        let mut r = Renderer::default();
        let mut output = Vec::new();
        r.draw(&e, (40, 8), &mut output).unwrap();
        assert!(
            output.windows(5).any(|s| s == b"38;2;"),
            "{}",
            String::from_utf8_lossy(&output)
        );
        output.clear();
        e.settings.termguicolors = false;
        r.draw(&e, (40, 8), &mut output).unwrap();
        assert!(!output.windows(5).any(|s| s == b"38;2;"));
        assert!(output.windows(5).any(|s| s == b"38;5;"));
    }

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
        assert_eq!(update.cursor, (6, 0, false));
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
        crossterm::style::force_color_output(true);
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
        let visible_line = |line: &str, left, width| {
            Line::visible(
                line,
                left,
                width,
                4,
                Highlight::default(),
                Highlight::default(),
                |_| false,
            )
            .text
        };
        assert_eq!(visible_line("a\u{1b}[31mb", 0, 40), "a?[31mb");
        assert_eq!(visible_line("a\tb", 0, 8), "a   b");
        assert_eq!(display_column("a界e\u{301}\t", 5, 4), 8);
        assert_eq!(visible_line("a界b", 2, 2), " b");
        assert_eq!(visible_line("e\u{301}", 0, 1), "e\u{301}");
    }
}
