mod buffer;
mod command;
mod editor;
mod install;
mod motion;

use buffer::Buffer;
use crossterm::{
    cursor::{Hide, MoveTo, SetCursorStyle, Show},
    event::{self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyEventKind},
    execute, queue,
    style::{Attribute, Print, ResetColor, SetAttribute},
    terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};
use editor::{Editor, Mode};
use std::env;
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use unicode_width::UnicodeWidthChar;

struct Terminal;

impl Terminal {
    fn enter() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        let guard = Self;
        execute!(
            io::stdout(),
            EnterAlternateScreen,
            EnableBracketedPaste,
            Hide
        )?;
        Ok(guard)
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = execute!(
            io::stdout(),
            ResetColor,
            SetCursorStyle::DefaultUserShape,
            SetAttribute(Attribute::Reset),
            Show,
            DisableBracketedPaste,
            LeaveAlternateScreen
        );
        let _ = terminal::disable_raw_mode();
    }
}

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

fn visible_cells(line: &str, left: usize, width: usize) -> Vec<(usize, String)> {
    let mut output = Vec::new();
    let mut column = 0;
    for (index, ch) in line.chars().enumerate() {
        let size = char_width(ch, column);
        let end = column + size;
        let text = if size == 0 {
            if column > left && column <= left + width {
                ch.to_string()
            } else {
                String::new()
            }
        } else if column >= left && end <= left + width {
            if ch == '\t' {
                " ".repeat(size)
            } else if ch.is_control() {
                "?".to_owned()
            } else {
                ch.to_string()
            }
        } else if end > left && column < left + width {
            " ".repeat(end.min(left + width) - column.max(left))
        } else {
            String::new()
        };
        if !text.is_empty() {
            output.push((index, text));
        }
        column = end;
        if column > left + width {
            break;
        }
    }
    output
}

fn visible_line(line: &str, left: usize, width: usize) -> String {
    visible_cells(line, left, width)
        .into_iter()
        .map(|(_, text)| text)
        .collect()
}

fn render(editor: &Editor, top: &mut usize, left: &mut usize) -> io::Result<()> {
    let buffer = &editor.buffer;
    let (width, height) = terminal::size()?;
    if width == 0 || height < 3 {
        return Ok(());
    }
    let rows = usize::from(height - 2);
    if buffer.row < *top {
        *top = buffer.row;
    } else if buffer.row >= *top + rows {
        *top = buffer.row + 1 - rows;
    }
    let cursor = display_column(&buffer.lines[buffer.row], buffer.col);
    let columns = usize::from(width);
    if cursor < *left {
        *left = cursor;
    } else if cursor >= *left + columns {
        *left = cursor + 1 - columns;
    }
    let mut out = io::stdout().lock();
    queue!(out, Hide, MoveTo(0, 0), Clear(ClearType::All))?;
    for screen_row in 0..rows {
        queue!(out, MoveTo(0, screen_row as u16))?;
        if let Some(line) = buffer.lines.get(*top + screen_row) {
            for (col, text) in visible_cells(line, *left, columns) {
                queue!(
                    out,
                    SetAttribute(if editor.selected(*top + screen_row, col) {
                        Attribute::Reverse
                    } else {
                        Attribute::Reset
                    }),
                    Print(text)
                )?;
            }
            if line.is_empty() && editor.selected(*top + screen_row, 0) {
                queue!(out, SetAttribute(Attribute::Reverse), Print(" "))?;
            }
            queue!(out, SetAttribute(Attribute::Reset))?;
        } else {
            queue!(out, Print("~"))?;
        }
    }
    let name = buffer
        .path
        .as_ref()
        .and_then(|p| p.file_name())
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "[No Name]".to_owned());
    let status = format!(
        " {} {}{} | {}:{} | Ctrl-S save  Ctrl-Q quit",
        editor.mode_name(),
        name,
        if buffer.dirty() { " [+]" } else { "" },
        buffer.row + 1,
        buffer.col + 1
    );
    queue!(
        out,
        MoveTo(0, height - 2),
        SetAttribute(Attribute::Reverse),
        Print(visible_line(&status, 0, columns)),
        SetAttribute(Attribute::Reset),
        MoveTo(0, height - 1),
        Print(visible_line(&editor.command_line(), 0, columns)),
        if editor.mode == Mode::Insert {
            SetCursorStyle::SteadyBar
        } else {
            SetCursorStyle::SteadyBlock
        },
        Show
    )?;
    if let Some(p) = &editor.prompt {
        let col = display_column(&format!("{}{}", p.kind, p.text), p.text.chars().count() + 1)
            .min(columns - 1);
        queue!(out, MoveTo(col as u16, height - 1))?;
    } else {
        queue!(
            out,
            MoveTo((cursor - *left) as u16, (buffer.row - *top) as u16)
        )?;
    }
    out.flush()
}

fn edit(path: Option<PathBuf>) -> io::Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(io::Error::other("Interactive editing requires a terminal."));
    }
    let mut editor = Editor::new(Buffer::open(path)?);
    let _terminal = Terminal::enter()?;
    let (mut top, mut left) = (0, 0);
    loop {
        editor.page_rows = usize::from(terminal::size()?.1.saturating_sub(2));
        render(&editor, &mut top, &mut left)?;
        match event::read()? {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                if editor.key(key) {
                    return Ok(());
                }
            }
            Event::Paste(text) => editor.paste(&text),
            _ => {}
        }
    }
}

fn run() -> io::Result<()> {
    let mut args = env::args_os().skip(1);
    let first = args.next();
    match first.as_deref().and_then(|s| s.to_str()) {
        Some("--version") => println!("fvim {}", env!("CARGO_PKG_VERSION")),
        Some("--help") | Some("-h") => println!(
            "fvim [file]\ni insert | Esc normal | :w save | :q quit | u undo | Ctrl-R redo\nCtrl-S save | Ctrl-Q quit (twice to discard)\n--version | --config-path"
        ),
        Some("--config-path") => println!("{}", install::config_dir()?.join("init.lua").display()),
        Some("--install") => install::install()?,
        Some("--init-config") => install::init_config()?,
        Some("--") => return edit(args.next().map(PathBuf::from)),
        Some(arg) if arg.starts_with('-') => {
            return Err(io::Error::other(format!("Unknown option: {arg}")))
        }
        _ => {
            if args.next().is_some() {
                return Err(io::Error::other("Open one file at a time; use :e to switch files."));
            }
            return edit(first.map(PathBuf::from));
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("fvim: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rendering_sanitizes_control_sequences() {
        assert_eq!(visible_line("a\u{1b}[31mb", 0, 40), "a?[31mb");
        assert_eq!(visible_line("a\tb", 0, 8), "a   b");
    }

    #[test]
    fn wide_and_combining_columns() {
        assert_eq!(display_column("a界e\u{301}\t", 5), 8);
        assert_eq!(visible_line("a界b", 2, 2), " b");
        assert_eq!(visible_line("e\u{301}", 0, 1), "e\u{301}");
    }
}
