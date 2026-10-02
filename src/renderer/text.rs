use crate::config::Highlight;
use crossterm::{
    queue,
    style::{Attribute, Color, Print, SetAttribute, SetBackgroundColor, SetForegroundColor},
};
use std::io::{self, Write};
use unicode_width::UnicodeWidthChar;

pub(super) fn char_width(ch: char, column: usize, tabstop: usize) -> usize {
    if ch == '\t' {
        tabstop - column % tabstop
    } else if ch.is_control() {
        1
    } else {
        ch.width().unwrap_or(1)
    }
}

pub(crate) fn display_column(line: &str, chars: usize, tabstop: usize) -> usize {
    line.chars()
        .take(chars)
        .fold(0, |col, ch| col + char_width(ch, col, tabstop))
}

#[derive(Clone, Default, PartialEq)]
pub(super) struct Line {
    pub(super) text: String,
    pub(super) spans: Vec<(usize, Highlight)>,
}

impl Line {
    pub(super) fn add(&mut self, text: &str, style: Highlight) {
        if !text.is_empty() {
            if self.spans.last().is_none_or(|p| p.1 != style) {
                self.spans.push((self.text.len(), style));
            }
            self.text.push_str(text);
        }
    }

    pub(super) fn visible(
        line: &str,
        left: usize,
        width: usize,
        tabstop: usize,
        normal: Highlight,
        visual: Highlight,
        selected: impl Fn(usize) -> bool,
    ) -> Self {
        Self::styled_visible(line, left, width, tabstop, |index| {
            if selected(index) {
                visual
            } else {
                normal
            }
        })
    }

    pub(super) fn styled_visible(
        line: &str,
        left: usize,
        width: usize,
        tabstop: usize,
        mut style_at: impl FnMut(usize) -> Highlight,
    ) -> Self {
        let mut out = Self::default();
        let mut column = 0;
        let right = left.saturating_add(width);
        for (index, ch) in line.chars().enumerate() {
            let size = char_width(ch, column, tabstop);
            let end = column + size;
            let visible = if size == 0 {
                column > left && column <= right
            } else {
                end > left && column < right
            };
            if visible {
                let style = style_at(index);
                if size == 0 || (column >= left && end <= right && ch != '\t') {
                    let mut bytes = [0; 4];
                    out.add(
                        if ch.is_control() {
                            "?"
                        } else {
                            ch.encode_utf8(&mut bytes)
                        },
                        style,
                    );
                } else {
                    for _ in column.max(left)..end.min(right) {
                        out.add(" ", style);
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

    pub(super) fn crop(&self, left: usize, width: usize, tabstop: usize) -> Self {
        let mut positions = self.text.char_indices().enumerate().peekable();
        Self::styled_visible(&self.text, left, width, tabstop, |index| {
            while positions.peek().is_some_and(|(i, _)| *i < index) {
                positions.next();
            }
            let byte = positions
                .peek()
                .filter(|(i, _)| *i == index)
                .map_or(self.text.len(), |(_, (byte, _))| *byte);
            self.spans
                .iter()
                .rev()
                .find(|(start, _)| *start <= byte)
                .map_or(Highlight::default(), |(_, s)| *s)
        })
    }
    pub(super) fn append(&mut self, other: Self) {
        let offset = self.text.len();
        for (start, style) in other.spans {
            if self.spans.last().is_none_or(|p| p.1 != style) {
                self.spans.push((offset + start, style));
            }
        }
        self.text.push_str(&other.text);
    }

    pub(super) fn paint(&self, out: &mut impl Write, rgb: bool) -> io::Result<()> {
        for (index, &(start, style)) in self.spans.iter().enumerate() {
            let end = self.spans.get(index + 1).map_or(self.text.len(), |p| p.0);
            paint_style(out, style, rgb)?;
            queue!(out, Print(&self.text[start..end]))?;
        }
        Ok(())
    }
}

fn color(rgb: Option<(u8, u8, u8)>, indexed: Option<u8>, truecolor: bool) -> Color {
    match (rgb, indexed, truecolor) {
        (Some((r, g, b)), _, true) => Color::Rgb { r, g, b },
        (_, Some(n), _) => Color::AnsiValue(n),
        // Custom RGB highlights also work in indexed mode, using the color cube.
        (Some((r, g, b)), _, false) => Color::AnsiValue(
            16 + 36 * ((u16::from(r) * 5 + 127) / 255) as u8
                + 6 * ((u16::from(g) * 5 + 127) / 255) as u8
                + ((u16::from(b) * 5 + 127) / 255) as u8,
        ),
        _ => Color::Reset,
    }
}

pub(super) fn paint_style(out: &mut impl Write, style: Highlight, rgb: bool) -> io::Result<()> {
    queue!(
        out,
        SetAttribute(Attribute::Reset),
        SetForegroundColor(color(style.fg, style.ctermfg, rgb)),
        SetBackgroundColor(color(style.bg, style.ctermbg, rgb))
    )?;
    for (enabled, attr) in [
        (style.bold, Attribute::Bold),
        (style.italic, Attribute::Italic),
        (style.underline, Attribute::Underlined),
        (style.reverse, Attribute::Reverse),
    ] {
        if enabled {
            queue!(out, SetAttribute(attr))?;
        }
    }
    Ok(())
}

pub(super) fn clipped(text: &str, width: usize) -> String {
    Line::visible(
        text,
        0,
        width,
        4,
        Highlight::default(),
        Highlight::default(),
        |_| false,
    )
    .text
}
