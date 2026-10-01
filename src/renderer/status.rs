use super::text::{clipped, display_column};
use crate::editor::Editor;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub(super) fn statusline(editor: &Editor, width: usize) -> String {
    let b = &editor.buffer;
    let path = b
        .path
        .as_ref()
        .map(|p| p.to_string_lossy())
        .unwrap_or_else(|| "[No Name]".into());
    let filename = b
        .path
        .as_ref()
        .and_then(|p| p.file_name())
        .map(|p| p.to_string_lossy())
        .unwrap_or_else(|| "[No Name]".into());
    let mut left = String::new();
    let mut right = String::new();
    let mut align = false;
    let mut format = editor.settings.statusline.chars();
    while let Some(ch) = format.next() {
        let target = if align { &mut right } else { &mut left };
        if ch != '%' {
            target.push(ch);
            continue;
        }
        match format.next().unwrap_or('%') {
            '=' => align = true,
            'f' | 'F' => target.push_str(&path),
            't' => target.push_str(&filename),
            'm' => {
                if b.dirty() {
                    target.push_str("[+]");
                }
            }
            'M' => {
                if b.dirty() {
                    target.push('+');
                }
            }
            'l' => target.push_str(&(b.row + 1).to_string()),
            'c' => target.push_str(&(b.col + 1).to_string()),
            'v' => target.push_str(
                &(display_column(&b.lines[b.row], b.col, editor.settings.tabstop) + 1).to_string(),
            ),
            'L' => target.push_str(&b.lines.len().to_string()),
            'p' => target.push_str(&((b.row + 1) * 100 / b.lines.len()).to_string()),
            'P' => {
                if b.lines.len() <= editor.page_rows {
                    target.push_str("All");
                } else if b.row == 0 {
                    target.push_str("Top");
                } else if b.row + 1 == b.lines.len() {
                    target.push_str("Bot");
                } else {
                    target.push_str(&format!("{}%", (b.row + 1) * 100 / b.lines.len()));
                }
            }
            '%' => target.push('%'),
            _ => {} // Validated by the configuration boundary.
        }
    }
    let left: String = left
        .chars()
        .map(|ch| if ch.is_control() { '?' } else { ch })
        .collect();
    let right: String = right
        .chars()
        .map(|ch| if ch.is_control() { '?' } else { ch })
        .collect();
    let right = clipped(&right, width);
    let room = width.saturating_sub(right.width());
    // Keep the filename visible when an absolute path exceeds the available cells.
    let left = if left.width() > room && room > 1 {
        let suffix: String = left
            .chars()
            .rev()
            .scan(0, |cells, ch| {
                *cells += ch.width().unwrap_or(1);
                (*cells < room).then_some(ch)
            })
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        format!("<{suffix}")
    } else {
        clipped(&left, room)
    };
    format!(
        "{left}{}{right}",
        " ".repeat(width.saturating_sub(left.width() + right.width()))
    )
}
